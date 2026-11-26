# Research: Version signals for AI-agent config in source repositories and marketplace formats

## Metadata

**Date:** 2026-11-23
**Domain:** packaging
**Triggered by:** Discussion `.agents/discussions/artifact-mirroring.md` — a mirror capability must give each imported unit a version; this file reports which version signals exist and how reliable they are. No ranking, no recommendation.
**Expires:** 2027-05-23 (vendor docs and star counts move fast; re-verify before relying)

Marks: `[tree]` read from a clone, `[page]` read from a web page, `[search]` search summary only. All reads dated 2026-11-23. Does not repeat `research_artifact_mirroring_vendor.md` (source shapes) or `research_artifact_mirroring_adjacent.md` (distro schemes).

Method deviation: clones were `--filter=blob:none --single-branch` with FULL history (not depth 50), because commit cadence (item f) needs history. Commit counts below come from `git log -- <path>` on those clones `[tree]`. Version-field bump histories come from `git show <commit>:<file>`. GitHub REST API hit the unauthenticated rate limit mid-run, so Releases counts are partly from HTML release pages.

## Direct Answer

Version information is present in three layers, with very different coverage:

1. Repository-level semver git tags: 6 of 15 surveyed repositories. They version the whole repository, never an individual skill.
2. Per-unit declared version fields: common on **plugins** (a manifest exists), rare on **skills** (the standard defines none; only `metadata.version` free text). Skills carry a version in 3 of 15 repositories for most units (K-Dense, Vercel) or a minority (awesome-copilot, ECC, ruflo).
3. Nothing but the commit: 4 of 15 repositories give no tag, no manifest version and no per-unit field.

Declared versions drift from content in both directions (examples in Part 2d). Claude Code itself treats an absent version as "use the commit SHA" and a present one as "pin until it changes", which makes both failure modes visible to users.

## Part 1. Version fields the formats define

| Format | Where a version can be declared | Required | Grammar | Absent means | Update trigger |
|---|---|---|---|---|---|
| Agent Skills standard (agentskills.io) | No `version` field. Only `metadata` (string-to-string map); the spec example is `metadata: {author: example-org, version: "1.0"}` | No (metadata optional; key names "reasonably unique" recommended) | Free string; example `"1.0"` | Nothing defined; clients define their own | None defined by the spec `[page]` https://agentskills.io/specification |
| Claude Code `plugin.json` | `version` | No (only `name` required; `claude plugin validate` warns on missing) | "Version string, not checked against semver" | Version computed from source: `github`/`url`/`git-subdir` -> commit SHA shortened to 12 chars (git-subdir also hashes the subdir path); `archive` -> 12 chars of sha256; relative path in git marketplace -> commit SHA of the directory; local non-git dir -> `unknown`; `npm` -> `unknown` | Computed version differs from `installed_plugins.json`. "Setting it pins the plugin to that version until you change it" `[page]` https://code.claude.com/docs/en/plugins-reference , https://code.claude.com/docs/en/plugins/loading.md |
| Claude Code marketplace entry | `plugins[].version`; top-level `version`; `metadata.version` (alternate location, "Marketplace manifest version") | No | Free string | Falls through to source-derived version | Precedence: `plugin.json` version first, then entry `version`, then source-derived. `validate` warns "Entry declares version x but plugin.json says y. At install time, plugin.json wins" `[page]` https://code.claude.com/docs/en/plugins/marketplace-reference.md |
| Claude Code source pin | `source.ref` (branch or tag, default branch if omitted), `source.sha` (full 40-char commit; wins over `ref` when both set). `npm` source has `version` (semver range or dist-tag). `archive` has `sha256` | No | git refs / SHA | Floating on default branch | Pinned sha moves only when the marketplace entry changes `[page]` same marketplace-reference URL |
| `command` plugin source | Version always derived from command output hash (`<manifest version>-<hash>` when manifest sets one); entry `version` ignored | n/a | hash | n/a | Re-run per session `[page]` loading.md |
| Copilot CLI plugins | `plugin.json` `version`; `marketplace.json` entry `version` ("Plugin version") | No ("Semantic Versioning is recommended") | Free, semver recommended | Not stated | Docs do not tie `version` to update logic; `copilot plugin update`, auto-update for first-party; GitHub/URL sources accept `sha` ("full 40-character commit SHA", "pin to a sha for reproducible installs") `[page]` https://docs.github.com/en/copilot/reference/copilot-cli-reference/cli-plugin-reference |
| VS Code agent plugins | `plugin.json` `version` ("Semantic Versioning is recommended"); marketplace entry "if applicable" | No | Free, semver recommended | Not stated | Authoring step: "Before publishing an update, bump the version field". npm/PyPI-sourced plugins never auto-update `[page]` https://code.visualstudio.com/docs/agents/reference/agent-plugin-reference , https://code.visualstudio.com/docs/copilot/customization/agent-plugins |
| Cursor plugins | `.cursor-plugin/plugin.json` `version` (example `"1.0.0"`); `marketplace.json` exists, fields not documented on the page | No ("only require a `name`") | Unspecified | Not stated | Not tied to `version`: GitHub-imported marketplaces auto-refresh "whenever changes are pushed to the branch the marketplace tracks" (at most every 10 min) `[page]` https://cursor.com/docs/plugins |
| Cursor rules (`.mdc`) | None documented (frontmatter in the surveyed repo: `description`, `globs`, `alwaysApply`) | n/a | n/a | n/a | n/a `[page]` cursor.com/docs/plugins, `[tree]` PatrickJS/awesome-cursorrules |
| Codex plugins | `.codex-plugin/plugin.json` root `version` (examples always include it; "keep portable identity ... such as name, version, description at the root"); marketplace entries have no plugin-level `version` except npm entries (optional: versions, dist-tags, ranges). Git entries take `ref` or `sha` | Not stated as required | Unspecified | Local plugins use cache path `$VERSION` = `local` | Not detailed on the page `[page]` https://developers.openai.com/codex/plugins/build |
| Codex skills (`openai/skills`) | No version field; `agents/openai.yaml` has `interface`/`dependencies`, 0 `version` hits | n/a | n/a | n/a | n/a `[tree]` |
| Gemini CLI extensions | `gemini-extension.json` `version` ("The version of the extension"; example `1.0.0`) | Page does not say | No grammar | n/a | `gemini extensions update` "to the version specified in its gemini-extension.json"; install takes `--ref` (branch/tag/commit); `--pre-release`, `--auto-update` flags; page does not say how update compares refs `[page]` https://geminicli.com/docs/extensions/reference/ |
| OpenCode skills | No `version` in SKILL.md frontmatter (recognised: `name`, `description`, `metadata.opencode/autoinvoke`, `disable-model-invocation`, `license`, `compatibility`). `version` appears only in an HTTP skill catalog `index.json`, where incrementing it refreshes the cache | n/a | n/a | n/a | Catalog `version` bump `[page]` https://opencode.ai/docs/skills/ |
| OpenCode plugins | Not covered by the page read | - | - | - | - (not determined) |
| Kiro powers | Current format: `plugin.json` with `version` among required fields ("Semantic version of the power, e.g. 1.0.0"). Legacy `POWER.md` still works; its frontmatter is not documented on the page | Yes (current format) | semver by description; no formal grammar | n/a | Page does not say how updates are detected `[page]` https://kiro.dev/docs/powers/create/ |
| Windsurf (Devin Desktop) skills | SKILL.md frontmatter: `name`, `description` only; no plugin manifest described | n/a | n/a | n/a | n/a `[page]` https://docs.devin.ai/desktop/cascade/skills (docs.windsurf.com redirects here) |
| `skills-lock.json` (`skills` CLI, vercel-labs/skills) | Lock v2 per skill: `computedHash` (SHA-256 over the installed files, CRLF normalised, excluding `metadata.json`/`.git`), `commit` (git sources only). Records no semver | n/a | hash / SHA | v1 lock upgraded on read | `update` "moves `commit` forward"; `install` fetches the recorded commit; `--frozen-lockfile` compares `computedHash` to disk. README has no versioning scheme `[page]` https://github.com/vercel-labs/skills |
| MCP registry `server.json` (contrast) | `version` | MUST be defined | "Recommends semantic versioning but supports any version string"; ranges (`^1.2.3`, `1.x`, `>=`) prohibited; must be unique per publication and immutable once published; unparsable strings always become "latest"; prerelease suffix recommended for metadata-only republish | n/a | New publication `[page]` https://modelcontextprotocol.io/registry/versioning |

## Part 2. What 15 repositories actually do

Stars/pushed from the GitHub API on 2026-11-23 `[page]` (api.github.com/repos/...). Two seeds moved: `ruvnet/claude-flow` is now `ruvnet/ruflo`; `affaan-m/everything-claude-code` is now `affaan-m/ECC`. Added to reach 15: `ComposioHQ/awesome-claude-skills`, `anthropics/knowledge-work-plugins`, `K-Dense-AI/scientific-agent-skills`. Tag lists from `git ls-remote --tags --refs` `[tree]`.

| # | Repository | Stars | Tags (scheme, count) | Releases | Per-unit version fields | Marketplace entries |
|---|---|---|---|---|---|---|
| 1 | anthropics/skills | 199k | 0 | 0 (API) | 0/20 SKILL.md | 5 entries, relative path, no entry version, no sha/ref; `metadata.version` "1.0.0" static since 2025-10-16 |
| 2 | anthropics/claude-plugins-official | 48k | 0 | 0 (API) | 37/57 plugin.json have `version`; 13/57 SKILL.md `version` | 317 entries: 14 with `version`; 261 of 262 object sources carry `sha` (164 sha only, 97 sha+ref); 55 relative path; 1 `command` source unpinned |
| 3 | obra/superpowers | 325k | 39 `vX.Y.Z` (v3.1.0..v7.0.2) | 17 (API) | 0/15 SKILL.md; one plugin, 7 manifests all 7.0.2 | 1 entry per manifest file, `version` 7.0.2 in 2 of 3 files |
| 4 | wshobson/agents | 43k | 0 | 0 (API) | 144/144 plugin.json with `version`; 16/185 SKILL.md (14 top-level, 2 nested) | 93 entries: 93 `version`; 91 relative path, 2 git-subdir (1 with sha) |
| 5 | github/awesome-copilot | 41k | 0 | not read | 31/482 SKILL.md (nested `metadata.version`); agents 4/229; instructions 0/195; plugin.json 115/115 | 235 entries: 235 `version`; 120 `github` sources (109 sha; 11 ref only; 45 sha+ref), 115 relative |
| 6 | vercel-labs/agent-skills | 35k | 6, named `agent-skills-<40-hex sha>`, 2026-08-21..2026-11-20 | not read | 9/9 SKILL.md nested `metadata.version` | no marketplace file |
| 7 | PatrickJS/awesome-cursorrules | 41k | 0 | 0 observed | 0/257 `.mdc` | none |
| 8 | openai/skills | 29k | 0 | not read | 0/44 SKILL.md | none |
| 9 | bmad-code-org/BMAD-METHOD | 56k | 169: 109 `vX.Y.Z`, 16 `vX.Y.Z-alpha.N`, 7 `X.Y.Z-alpha.N` (no `v`), 8 beta in two casings, 1 `web-bundles-vX.Y.Z` | many (5+ pages; latest v6.12.4) | 0/36 SKILL.md | none; root `CHANGELOG.md` |
| 10 | ruvnet/ruflo (was claude-flow) | 82k | 1761: 912 `task-N-N`, 519 `session-session-N-N`, 302 `vX.Y.Z`, 19 alpha, 3 scoped `@claude-flow/<pkg>@X.Y.Z`, 1 `ruflo-console-vX.Y.Z`, rest ad hoc | many (latest v3.80.4) | 56/56 plugin.json; 19/403 SKILL.md | 49 entries, 0 entry `version`; 3 sha+ref |
| 11 | SuperClaude-Org/SuperClaude_Framework | 24k | 19 `vX.Y.Z` (v1.0.0..v4.3.0, last 2026-03-22) | 10+ (latest v4.3.0) | 0/9 SKILL.md; `VERSION`, `pyproject.toml`, `plugin.json` all 4.3.0 | none |
| 12 | affaan-m/ECC (was everything-claude-code) | 310k | 20 `vX.Y.Z` (v0.6.0..v2.2.3, last 2026-10-01) | 10+ (latest v2.2.3) | 82/1089 SKILL.md (44 top-level, 38 nested; 69 of the values are `1.0.0`); plugin.json 7/7 = 2.2.3 | 1 entry per file, `version` set |
| 13 | ComposioHQ/awesome-claude-skills | 80k | 0 | not read | 0/864 SKILL.md | 107 entries relative, 0 entry `version`; marketplace top-level `version` 2.0.0 |
| 14 | anthropics/knowledge-work-plugins | 37k | 0 | 0 (API) | 22/22 plugin.json; 2/252 SKILL.md | 123 entries: 0 `version`; 101 of 101 object sources carry `sha` (42 also `ref`); 22 relative |
| 15 | K-Dense-AI/scientific-agent-skills | 58k | 109 `vX.Y.Z` (v1.50.0..v2.72.0, 2025-10-22..2026-10-01) | 11+ pages (latest v2.72.0, 2026-10-01) | 177/177 SKILL.md nested `metadata.version` (e.g. `"1.6"`) plus `last-reviewed` | none; one root `plugin.json` 2.72.0 |

Sources for all rows: `[tree]` clones of each repository on 2026-11-23, tags via `git ls-remote`, stars via api.github.com, Releases for rows 9, 11, 12, 10, 15 via `github.com/<repo>/releases` `[page]`.

### a. Tags and Releases: what they version

- Whole-repository only. In every repo with semver tags, one tag covers all skills in the repository. No `skill/1.2.3` or `plugin@1.2.3` scheme exists for skills or plugins in these 15.
- Component tags exist only for non-skill artifacts: ruflo (`@claude-flow/codex@3.0.3`, `@claude-flow/swarm@3.0.2`, `@claude-flow/plugins@3.0.1`, `ruflo-console-v0.43.0`) and BMAD (`web-bundles-v1.0.0`) `[tree]`.
- Scheme noise inside one repo: BMAD mixes `v6.0.0-Beta.2`, `6.0.0-Beta.3` (no `v`), `-beta.N`, `-alpha.N`; ruflo's 1761 tags are 68% automation tags (`task-*`, `session-session-*`) rather than versions.
- Vercel tags embed the full commit sha in the tag name (`agent-skills-6ec2693...`), 6 tags in 3 months; not semver.
- Tag vs content: SuperClaude has 5 commits after v4.3.0 (2026-03-22 tag, last commit 2026-09-27); ECC has 2132 commits after v2.2.3 (tag 2026-10-01, last commit 2026-11-19) `[tree]` `git rev-list --count <tag>..HEAD`. Tag cadence is therefore not commit cadence.
- Tags not on the default branch: in BMAD, `git describe` at default-branch HEAD returns `v6.12.0-273-ge435a225` while the remote lists v6.12.1..v6.12.4 (latest release v6.12.4), so the newest release tags are not ancestors of the default branch HEAD in this single-branch clone `[tree]`. Cause not investigated.
- Tag vs in-repo manifest (obra/superpowers, 39 tags): `plugin.json` version equals the tag in 36; 3 differ (v3.1.0 has manifest 3.0.1, v3.3.1 has 3.2.3, v4.1.0 has 4.0.3) `[tree]`.
- Release-note cadence: obra/superpowers tags v7.0.0 (2026-10-09), v7.0.1 (2026-10-21), v7.0.2 (2026-11-03), each created by a "Release vX.Y.Z" commit that also edits the manifest `[tree]`. ruflo `v3*` tags: 24 in 2026-09, 64 in 2026-10, 31 in 2026-11 (to the 23rd).

### b. Per-unit version fields filled

- Skills, standard frontmatter: no top-level `version`; where present it is `metadata.version` free text (`"1.0"`, `"1.6"`, `"4.0.0"`) or an undeclared top-level `version:` key. Coverage across 15 repos, `[tree]`: anthropics/skills 0/20; plugins-official 13/57 skill files; superpowers 0/15; wshobson 16/185; awesome-copilot 31/482; vercel 9/9; cursorrules 0/257; openai/skills 0/44; BMAD 0/36; ruflo 19/403; SuperClaude 0/9; ECC 82/1089; composio 0/864; knowledge-work 2/252; K-Dense 177/177.
- ECC: of 82 filled values, 69 are `1.0.0`; skills with `1.0.0` that I checked have 3 to 6 commits on the directory.
- Non-skill unit types: awesome-copilot agents 4/229 carry `version:`; instructions 0/195; cursor rules 0/257. Their frontmatter fields are `name`, `description`, `model`, `tools` (agents) and `description`, `globs`, `alwaysApply` (rules) `[tree]`.
- Plugin manifests carry versions much more often: wshobson 144/144, awesome-copilot 115/115, ruflo 56/56, knowledge-work 22/22, official 37/57.
- Marketplace entries that restate `version`: wshobson 93/93, awesome-copilot 235/235, official 14/317, knowledge-work 0/123, ruflo 0/49, composio 0/107.

### c. Marketplace entries: pinned or floating

- plugins-official: 261 of 262 git-based entries carry a full `sha`; the 55 in-repo entries ride the marketplace commit; the one `command` entry is unpinned. Entry `version` only in 14 of 317.
- knowledge-work-plugins: 101 of 101 git-based entries carry `sha`; 0 entry `version`.
- awesome-copilot external entries: 109 of 120 have `sha`; 11 have only `ref` (floating). Example: `ref: copilot-plugin-v7.1.0-aaf1cfb` plus a 40-char `sha`, i.e. the upstream repository publishes a tag whose name carries a version and a short sha `[tree]`.
- anthropics/skills: relative-path entries with `strict: false` and a `skills` list; no `version`.
- Whole-marketplace version string exists in 4 repos (`metadata.version`: anthropics/skills "1.0.0", wshobson "1.7.1", awesome-copilot "1.0.0"; top-level `version`: composio "2.0.0"); in anthropics/skills it has not changed since the first commit of the file (2025-10-16), while the file was edited 9 times through 2026-08-18 `[tree]`.

### d. Do declared versions move when content moves? (`git log` on the unit directory vs the version field's history)

| Unit | Directory commits | Version history | Reading |
|---|---|---|---|
| vercel `skills/react-best-practices` `[tree]` | 58 | `version: "1.0.0"` since 2026-01-15, never changed | 50 commits after the only value; content moves, version does not |
| vercel `skills/react-view-transitions` `[tree]` | 74 | `"1.0.0"` since 2026-03-29, never changed | 61 commits after the last value |
| ECC `skills/continuous-learning-v2` `[tree]` | 142 | `2.1.0` set 2026-08-12 | 48 directory commits after that value |
| anthropics/claude-plugins-official `plugins/code-modernization` `[tree]` | 62 | `1.0.0` (2026-09-25), `1.1.0` (2026-11-19) | two values for 62 directory commits |
| plugins-official `plugins/plugin-dev` `[tree]` | 12 | `0.1.1` (2026-10-26), `0.1.2` (2026-11-10) | 2 values / 12 commits |
| plugins-official `plugins/claude-security` `[tree]` | 8 | 7 values in 2026-07-22..2026-09-25 (`0.10.0`, `0.10.1-rc7`, `0.10.1`, `0.10.2`, `0.10.2.3`, `0.11.0`, `0.12.0`) | roughly one value per commit; non-semver 4-part `0.10.2.3` and `-rc7` in a field Claude Code does not check |
| plugins-official `plugins/security-guidance` `[tree]` | 334 | 24 value changes, including `1.0.0` then a commit with no version (2025-12-01) then `2.0.0` on 2026-05-26 | version field removed and re-added; 19 directory commits after the last bump read |
| wshobson `plugins/plugin-eval` `[tree]` | 112 | `0.1.1` -> `0.1.5` over 2026-05-22..2026-11-18 (6 values) | 112 commits / 6 values; repo CONTRIBUTING requires a patch bump per changed plugin file, enforced by `make check-version-bumps` (CONTRIBUTING.md lines 29-34) |
| wshobson `plugins/python-development` `[tree]` | 30 | `1.2.1`..`1.2.4`, 4 values over 2026-02-05..2026-10-30 | 30 commits / 4 values |
| K-Dense `skills/diffdock` `[tree]` | 9 | 9 values `1.0`..`1.6` (one value re-used), bumped in step with the commits | every directory commit coincided with a bump; the version is also a review marker (`last-reviewed: "2026-09-30"`) |
| ruflo `plugins/ruflo-console` / `ruflo-mods` `[tree]` | 503 / 110 | manifest touched in 137 / 53 commits; per-plugin `CHANGELOG.md` touched in 73 / 28; now 0.51.0 / 0.17.5 | manifest edits are about a quarter to a half of directory commits |
| obra/superpowers repository `[tree]` | per-skill median 11 commits in 12 months (max 76) | one repo-wide value 7.0.2 across 7 manifests, bumped only by "Release vX.Y.Z" commits | skill content changes between releases carry no per-skill version |

Bump without content change: not found in this sample except the security-guidance remove/re-add and ruflo manifest-only commits (manifest-touching commits exceed changelog-touching ones). The vd checks count commits to the version file; a commit can touch the file without changing the value, so the "values seen" counts are the lower bound reading of bumps.

### e. Other change signals

- Per-plugin `CHANGELOG.md`: ruflo has 51 `CHANGELOG*` files across the tree (per-plugin ones included); plugins-official has 1 (`plugins/code-modernization/CHANGELOG.md`); knowledge-work has 1; BMAD and SuperClaude have a root one. Release tooling configs (release-please, changesets, lerna): none found in any of the 15 by file-name search `[tree]`.
- `package.json`: present in nearly all (ECC 29, ruflo 54, awesome-copilot 34), usually for tooling not for the units.
- CI-enforced version policy: wshobson only (`make check-version-bumps`, CONTRIBUTING.md `[tree]`).
- Generated multi-vendor manifests: wshobson (`make generate-all` writes Codex and Cursor manifests from the Claude one), superpowers (`.claude-plugin`, `.muse-plugin`, `.agents/plugins`, `gemini-extension.json` carrying the same repo version) `[tree]`.
- Other metadata that is not a version but changes with review: K-Dense `last-reviewed`; Claude Code's `engines.claude-code` range on plugins `[page]`.

### f. Commit cadence on unit directories (rough)

Per skill directory, first up to 60 `SKILL.md` directories per repository, `git log --since` on the unit directory `[tree]`:

| Repo | Units sampled | Commits per unit, last 12 months: median / max | Last 6 months median | Units with 0 commits in 12 months |
|---|---|---|---|---|
| anthropics/skills | 20 | 2 / 31 | 0 | 0 |
| plugins-official | 57 | 1 / 6 | 1 | 1 |
| obra/superpowers | 15 | 11 / 76 | 8 | 0 |
| wshobson/agents | 60 | 4 / 10 | 0 | 0 |
| awesome-copilot | 60 | 1 / 7 | 0 | 0 |
| vercel-labs/agent-skills | 9 | 8 / 74 | 1 | 0 |
| openai/skills | 44 | 5 / 12 | 0 | 0 |
| BMAD | 36 | 1 / 4 | 1 | 0 |
| ruflo | 60 | 3 / 6 | 1 | 0 |
| ECC | 60 | 4 / 36 | 3 | 0 |
| composio | 60 | 1 / 1 | 0 | 11 |
| knowledge-work-plugins | 60 | 1 / 6 | 0 | 0 |
| K-Dense | 60 | 8 / 18 | 8 | 0 |

The median unit moves 1 to 11 times a year; the tail (anthropics/skills `claude-api` 31, vercel `react-view-transitions` 74, ECC `skill-comply` 36, superpowers `brainstorming` 76) moves weekly. Commits include renames, formatting and bulk cross-repo edits, so a commit-derived version would also move on edits that do not change the unit's meaning. Not measured: how many of these commits touch only `SKILL.md` frontmatter or non-instruction files.

## Part 3. Summary counts (N = 15)

| Signal | Count | Repositories |
|---|---|---|
| Repository-level semver tags | 6 | superpowers, BMAD, ruflo, SuperClaude, ECC, K-Dense |
| Tags of another shape (sha-named) | 1 | vercel (`agent-skills-<sha>`) |
| Per-unit (skill or plugin) tags | 0 | none; component tags exist for non-skill artifacts in ruflo and BMAD |
| Per-unit version fields filled for most units (plugin manifest or skill metadata) | 6 | wshobson (plugins), awesome-copilot (plugins), ruflo (plugin.json), knowledge-work (plugin.json), K-Dense (skills), vercel (skills); plugins-official partial (37/57 manifests) |
| Per-unit fields filled for most skills (not plugins) | 2 | K-Dense 177/177, vercel 9/9 |
| Whole-marketplace version string only | 3 | anthropics/skills (static), composio (2.0.0), wshobson (1.7.1, also per entry) |
| Nothing beyond the commit (no tag, no manifest version, no skill field) | 4 | anthropics/skills, cursorrules, openai/skills, composio |
| Sha-pinned marketplace entries | 3 | plugins-official 261/262, knowledge-work 101/101, awesome-copilot 109/120 |

Cases where each signal would give a wrong or missing version:

- Repository semver tag: missing in 9 of 15 repos; covers all units at once so one skill change bumps every unit; lags content (ECC 2132 commits after the last tag, SuperClaude 5 commits and 6 months); mixed schemes (BMAD `v`/no-`v`, `-Beta`/`-beta`, `web-bundles-v`); noise tags (ruflo 68% automation tags); newest tags off the default branch (BMAD in the single-branch clone); tag vs manifest mismatch in 3 of 39 (superpowers); sha-named tags (vercel).
- Plugin manifest `version`: free text and unchecked by Claude Code (`0.10.2.3`, `0.10.1-rc7`); can be removed (security-guidance 2025-12-01); two sources disagree (manifest wins over entry; `validate` warns); fixed while content moves (marketplace metadata `1.0.0` in anthropics/skills; code-modernization 2 values/62 commits); pinned by author so consumers see no update while content changes (Claude Code semantics `[page]`).
- Skill `metadata.version`: defined by no standard field, 0 coverage in 9 of 15 repos, constant `1.0.0` while directories move 50+ commits (vercel, ECC), `"1.0"` vs `"1.0.0"` shapes mixed across ECC, K-Dense uses it as a review counter bumped on nearly every commit.
- Marketplace entry `version`: absent in 3 of 6 marketplaces that list git sources (plugins-official 303/317 missing, knowledge-work 123/123, ruflo 49/49); copy of the manifest value, so can disagree with it.
- Marketplace `sha`/`ref`: `sha` is an exact content pin but carries no ordering or name; `ref`-only entries float (11 in awesome-copilot); a pinned `ref` tag name sometimes embeds a version (`copilot-plugin-v7.1.0-aaf1cfb`) but that is upstream's choice.
- Commit SHA (Claude Code fallback): moves on every commit to the repo for relative-path plugins; `git-subdir` version adds a hash of the subdir path, so it is stable across unrelated repo commits only if the SHA of the source is the subdirectory's tree (the page says "commit SHA of the source", not the subdir tree) — not verified.

## What I could not determine

- GitHub Releases counts for awesome-copilot, vercel, openai/skills, cursorrules, composio (API rate limit; HTML page scraping used only for BMAD, K-Dense, ECC, SuperClaude, ruflo, and none gave a total).
- Whether Claude Code's `git-subdir` fallback version tracks the subdirectory tree or the whole-repo commit.
- OpenCode plugin versioning, Codex skills update behaviour, Kiro legacy `POWER.md` frontmatter, Cursor `marketplace.json` fields, how Gemini compares refs on update (pages silent).
- Gemini CLI extension repositories, Kiro powers repositories and Windsurf-specific repositories were not sampled; the 15 repositories are skill/plugin heavy, and Claude-oriented.
- Why BMAD's newest tags are not reachable from the default branch.
- Commit cadence for non-skill units (agents, rules, instructions, commands) and for plugins as a whole.
- Whether the sampled commits are content or housekeeping.

## Sources

| Source | Type | Date read | Relevance |
|---|---|---|---|
| https://agentskills.io/specification | Docs | 2026-11-23 | skill frontmatter, `metadata.version` |
| https://code.claude.com/docs/en/plugins-reference | Docs | 2026-11-23 | `plugin.json` `version` |
| https://code.claude.com/docs/en/plugins/loading.md | Docs | 2026-11-23 | version computation, update triggers, cache dir |
| https://code.claude.com/docs/en/plugins/marketplace-reference.md | Docs | 2026-11-23 | entry/metadata version, `ref`/`sha`, validation warnings |
| https://docs.github.com/en/copilot/reference/copilot-cli-reference/cli-plugin-reference | Docs | 2026-11-23 | Copilot CLI plugin version and `sha` |
| https://code.visualstudio.com/docs/agents/reference/agent-plugin-reference | Docs | 2026-11-23 | VS Code plugin version |
| https://cursor.com/docs/plugins | Docs | 2026-11-23 | Cursor plugin manifest, refresh |
| https://developers.openai.com/codex/plugins/build | Docs | 2026-11-23 | Codex plugin manifest, `ref`/`sha` |
| https://geminicli.com/docs/extensions/reference/ | Docs | 2026-11-23 | `gemini-extension.json` |
| https://kiro.dev/docs/powers/create/ | Docs | 2026-11-23 | Kiro power `plugin.json` |
| https://opencode.ai/docs/skills/ | Docs | 2026-11-23 | OpenCode skill frontmatter |
| https://docs.devin.ai/desktop/cascade/skills | Docs | 2026-11-23 | Windsurf skills |
| https://github.com/vercel-labs/skills | Repo | 2026-11-23 | `skills-lock.json` |
| https://modelcontextprotocol.io/registry/versioning | Docs | 2026-11-23 | `server.json` version rules |
| github.com/<owner>/<repo> clones (15 repos listed in Part 2) | Repo `[tree]` | 2026-11-23 | tags, manifests, frontmatter, cadence |
| github.com/{bmad-code-org/BMAD-METHOD, K-Dense-AI/scientific-agent-skills, affaan-m/ECC, SuperClaude-Org/SuperClaude_Framework, ruvnet/ruflo}/releases | Page | 2026-11-23 | latest release tags |

## negative:

- No skill-level tag scheme (`skill/1.2.3`, `name@1.2.3`) in any of 15 repositories.
- No release-please, changesets or lerna config found in any of the 15.
- No per-unit `version` in anthropics/skills, openai/skills, cursorrules, composio, BMAD, SuperClaude, superpowers skills.
- Agent Skills standard, OpenCode and Windsurf define no skill `version` field; Cursor rules define none.
- Codex marketplace entries carry no plugin-level `version` except npm entries.

## leads:

- git-subdir fallback version: test what Claude Code's 12-char SHA covers for a `git-subdir` source — one sentence of code or a controlled install would settle it.
- Subtree-hash derivation: the skills CLI `computedHash` and Claude Code's `git-subdir` hash are both content-derived; compare their stability against commit SHA on the repositories above.
- Automation-tag noise: examine ruflo's `task-*` and `session-session-*` tags to see whether any mirror tag filter would need an allow-list pattern.
- Release-branch tags: BMAD tags absent from main; check how a mirror would resolve "latest" there.
- Sample Gemini extension, Kiro power and OpenCode plugin repositories for the formats this survey could only read as docs.
- Measure how much of directory commit volume is frontmatter or housekeeping only.
