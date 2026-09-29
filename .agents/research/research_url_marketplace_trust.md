# Research: integrity and update properties of URL-hosted vs git-hosted harness marketplaces

## Metadata

Date: 2026-09-28 · Lane: neutral web scan (no recommendation) · Extends `research_marketplace_hosting_prior_art.md` (Tables 1-3) and frames against `adr_artifact_trust_model.md` (grim guarantees *integrity* by digest pin, not *authenticity*; registry/publisher-toolchain compromise is out of scope).
Forms compared: (A) git repo marketplace; (B) `marketplace.json` at an HTTPS URL (GitHub/GitLab Pages), entries need absolute sources.
Access date for all sources: 2026-09-28. WebFetch returns a small-model summary; "not stated" = absent from the summary, not proven absent upstream. Bash `curl` was denied, so no response headers were measured first-hand.
Sources: [CH] https://code.claude.com/docs/en/plugins/host-marketplace · [CL] .../plugins/loading · [CR] .../plugins/marketplace-reference · [CO] .../plugins/org · [CS] .../plugins/security (all code.claude.com/docs/en) · [J] https://junie.jetbrains.com/docs/junie-cli-extensions.html · [D] https://docs.factory.ai/cli/configuration/plugins · [Q] https://docs.qoder.com/cli/plugins · [X1] https://developers.openai.com/codex/plugins/build · [X2] https://learn.chatgpt.com/docs/enterprise/managed-configuration · [X3] https://github.com/openai/codex/issues/23478 · [G1] https://docs.github.com/en/copilot/reference/copilot-cli-reference/cli-plugin-reference · [G2] https://github.blog/changelog/2026-06-25-enterprise-managed-settings-now-support-strictknownmarketplaces-in-vs-code-and-the-cli/ · [U] https://cursor.com/docs/plugins · [P1] https://www.air.security/blog-posts/plugin4shell · [P2] https://www.helpnetsecurity.com/2026/09/18/plugin4shell-ai-coding-agents-vulnerability/ (search snippet only) · [I1] https://github.com/anthropics/claude-code/issues/37340 · [I2] https://promptarmor.substack.com/p/hijacking-claude-code-via-injected · [W1] https://docs.github.com/en/pages/configuring-a-custom-domain-for-your-github-pages-site/verifying-your-custom-domain-for-github-pages · [W2] https://gitlab.com/gitlab-org/gitlab/-/issues/464558 · [W3] https://github.com/orgs/community/discussions/11884 · [H] https://docs.brew.sh/Homebrew-Security-and-Supply-Chain · [V1] https://www.securityweek.com/open-vsx-publisher-account-hijacked-in-fresh-glassworm-attack/ · [V2] https://www.bleepingcomputer.com/news/security/open-vsx-rotates-tokens-used-in-supply-chain-malware-attack/ · [PF] https://cside.com/blog/polyfill-supply-chain-attack-timeline · [E] https://thehackernews.com/2026/03/chrome-extension-turns-malicious-after.html

## Q1 - Integrity: what is verified on fetch, what can be pinned

| Layer | (A) git marketplace | (B) URL marketplace | Source |
|---|---|---|---|
| Marketplace pin, Claude | `#ref` (branch/tag) on `marketplace add`; `ref`+`path` in settings source. No `sha` field documented for a *marketplace* source (github/git fields: repo/url, ref, path, sparsePaths) | **None.** `url` source fields: `url`, `headers`, `headersHelper`. The catalog is whatever the URL serves at fetch time. TLS only | CR, CH |
| Marketplace pin, others | Droid: `#<ref>` or `@<40-hex sha>` on `marketplace add`. Codex: `--ref`. Copilot/Cursor/Junie/Qoder: not stated | Droid/Junie/Qoder accept a bare URL; no pin, checksum or signature stated in D, J, Q | D, X1, J, Q |
| Plugin-entry pin, Claude | `github`/`url`/`git-subdir`: `ref` + 40-hex `sha`; with both set, `sha` wins and install survives a deleted branch | `archive`: `sha256` (64 hex). "When you set it, Claude Code refuses a download that doesn't match" (error "Plugin archive integrity check failed"). **`sha256` is optional**; without it the version is the digest of whatever was downloaded | CR, CS |
| Plugin-entry pin, others | Copilot: `ref`, `sha` ("immune to force-pushes"). Codex: `ref`/`sha` selectors in git sources | Not stated for archive/URL kinds | G1, X1 |
| Where the pin lives | In the catalog. In both forms the catalog owner can rewrite pin and target together; a pin defends against a plugin host changing bytes under a fixed catalog, not against the catalog owner | Same. If catalog and zips share one Pages origin, one compromise changes JSON, zip and `sha256` together; if zips sit elsewhere with a `sha256` in the Pages JSON, only a Pages compromise defeats it | inference from CR |

Other verified facts:
- Claude community catalog pins nearly every entry to a commit SHA and "refuses to install a different commit" [CS]. That is a catalog-content property, not a hosting-form property.
- `archive` URL must be `https://` and may not target loopback, link-local or cloud-metadata hosts [CR]. Marketplace-level `headers` go only to the marketplace URL's origin; a redirect off-origin drops them [CH].
- **Plugin4Shell** (disclosed 2026-09-17, reported June): agents checked out a "pinned" SHA but did not confirm the working tree equalled it; an attacker-controlled repo could make a branch named like the SHA. Fixed in Claude Code 2.1.179 (June 17), Codex 0.146.0; Copilot unpatched at disclosure; Gemini CLI will not be fixed. Scope named: git sources (GitHub partly protected, Bitbucket and self-hosted vulnerable). Archive/`sha256`/npm not mentioned in [P1]. Quote: "No marketplace can enforce it - the fix has to ship in the agent." [P1]
- Copilot, Cursor, Junie, Qoder: no integrity check documented for any source (G1, U, J, Q).

## Q2 - Update semantics

| Harness | Default | Refresh trigger and cadence | Compare rule | Source |
|---|---|---|---|---|
| Claude, third-party marketplace (either form) | auto-update **off**; on for official and claude.ai-added marketplaces. `autoUpdate` per `extraKnownMarketplaces` entry; managed value locks the toggle | If on: first message, random delay up to 10 min, then refresh. Always refreshes the catalog before `install name@marketplace` (skipped within 30 s, for local `file`/`directory`, seeds). Manual: `/plugin marketplace update`, `claude plugin update`. Refresh failure: install proceeds from cached catalog ("marketplace not refreshed") | Manifest `version` > entry `version` > source-derived (git SHA[:12]; archive sha256[:12] or digest of the download; npm/local `unknown`). "Pinned `version` + new commits = no update"; a changed `sha256` alone therefore does not update a plugin whose manifest/entry sets `version` (inference from rule order) | CL, CH |
| Claude, git marketplace background check | same toggle | Checks for new commits; on new commits **or an unreachable/unauthenticated remote** it re-clones and replaces the checkout; re-clone failure keeps the old one. `CLAUDE_CODE_PLUGIN_KEEP_MARKETPLACE_ON_FAILURE=1` keeps it without re-clone | as above | CH |
| Claude, URL marketplace | same toggle | Stored under `marketplaces/<name>/`. **Whether conditional requests (ETag, If-None-Match, Cache-Control) are honoured is not stated** | as above | CL |
| Copilot CLI | first-party auto-updates; custom marketplaces opt in via `autoUpdate: true` (interactive and `-p` only) | `copilot plugin update NAME/--all`; compared value not documented | not stated | G1 |
| Codex | best-effort auto-upgrade of configured **git** marketplaces (PR #17425, tracks `last_revision`); `codex plugin marketplace upgrade [name]` | Cooldown request "minimum_release_age" open since 2026-05-19 [X3] | not stated | X1, X3 |
| Droid | auto-sync may skip a recently checked marketplace "for up to six hours"; `droid plugin marketplace update` immediate | git installs tracked by commit hash | commit | D |
| Junie | **manual only** ("select it in the Installed tab and choose Update") | GitHub `/blob/` URLs rewritten to raw | not stated | J |
| Qoder | `qoder plugins marketplace update` refreshes catalog; auto-update not stated | | semver (prior research) | Q |
| Cursor | GitHub imports auto-refresh, re-index at most every 10 min; admin manual Refresh | git only | not stated | U |

Entry disappears or pin changes (Claude, the only harness documenting it):
- Removed entry, no `renames`/`forceRemoveDeletedPlugins`: plugin **stays installed**, reports `Plugin "<name>" not found in marketplace`. With `forceRemoveDeletedPlugins: true` it is uninstalled from user/project/local scope at next session start (managed-installed plugins stay), listed under Flagged. `renames` maps old name to new or `null`. [CH]
- `sha256` changes: derived version changes (when no `version` field), new archive is downloaded and checked against the new pin; the old version dir is orphaned and swept after 14 days. No prompt is documented for a non-`command` source. [CL]
- Marketplace `name` change: not stated. Marketplace **removed**: uninstalls its plugins (Claude, Qoder). [CS, Q]
- Codex: an entry whose source cannot be resolved "is skipped" rather than failing the marketplace; effect on installed plugin not stated. [X1]
- Droid, Junie, Copilot, Cursor: behaviour on removed entry not stated.

## Q3 - Pages-specific takeover and staleness risks

- **GitHub Pages custom domain**: GitHub states takeover "can happen when you delete your repository, when your billing plan is downgraded, or after any other change which unlinks the custom domain or disables GitHub Pages while the domain remains configured ... and is not verified"; verification "stops other GitHub users from taking over your custom domain". [W1] A `github.io` URL has no DNS to dangle; its risk is account/org name loss (see repojacking under Q5).
- **GitLab Pages**: a dangling custom domain served content before verification, "allowing 7 days before disabling it"; reported 2024-05-28, HackerOne #2523654, listed open in [W2] (status as fetched). Namespace reuse: search snippets (not fetched) say GitLab makes a renamed namespace available immediately and warns of malicious replacement via stale links (gitlab-ce#44925, gitlab-foss#43023); treat as unverified.
- **Stale JSON via cache**: a community post (not GitHub docs) says Pages sends `Cache-Control: max-age=600` [W3]; no GitHub statement on CDN purge delay was found. Consequence for consumers is only known for Claude's own rules: refresh happens at add, install-by-`name@marketplace`, and (if enabled) auto-update; conditional-request behaviour unstated. Time-to-effect of a takeback or a corrected pin therefore = origin cache + harness refresh cadence (Q2), and Junie/Qoder never refresh unprompted.
- **Mixed sources**: managed allow/blocklists match the *marketplace* source only, "not the plugin's own entry inside that marketplace" [CO]. An allowlisted Pages catalog can point entries at any `archive`, `npm`, `github` or `url` host; an allowlisted git catalog can too. The class of exposure is the same in A and B; B adds that the catalog itself has no commit history or ref semantics for a consumer to inspect (`marketplace add ...#ref` is not available for `url` sources).
- **Relative sources**: fail in URL form (Claude) [CH]; Junie/Qoder/Droid behaviour unstated, so a Pages catalog must be all-absolute on Claude, and a mixed git-hosted catalog can use relative paths (cloned whole tree).
- **Git form**: catalog integrity is git object integrity for whatever ref is fetched, but branch refs are mutable and Claude's `#ref` on a marketplace is not a SHA pin; Plugin4Shell shows the SHA verification itself was absent in four agents until patched.

## Q4 - Enterprise and org controls

| Harness | Control | Treatment of URL vs git | Source |
|---|---|---|---|
| Claude | `strictKnownMarketplaces` (alias `allowedMarketplaces`), `blockedMarketplaces`, `extraKnownMarketplaces`, `disableSideloadFlags`; checked before download **and again at session start for installed plugins** (fails with "not in the allowed marketplace list") | `url`: matches on the `url` string; `headers` not compared; trailing slash, `.git`, `ssh://` are different values. `github`/`git`: repo/url, `ref`, `path` must all match or be absent both sides; `owner/*` wildcard for GitHub. `hostPattern`: regex on host of `github`, `git`, `url` sources (anchor with `^...$`; a `github` source counts as `github.com`). Blocklist canonicalises git URLs but `url` entries are exact. `url` entries also block matching `https` repo URLs. `[]` blocks everything | CO, CR |
| Claude, extras | `enabledPlugins: false` hides/blocks a plugin from an allowed marketplace; `autoUpdate` and `DISABLE_AUTOUPDATER` policy; `pluginTrustMessage`; allowlist cannot "restrict entries inside an allowed marketplace" | | CO |
| Codex | `requirements.toml` `allowed_marketplaces`: git = normalised repo URL + optional exact `ref`; host regex on lowercase git host; local = absolute path. Rejects unmatched add, install and configured-git refresh; filters at runtime | No `url` rule type documented (Codex documents git and local marketplaces only) | X2, X1 |
| Copilot CLI / VS Code | enterprise-managed `strictKnownMarketplaces`, `extraKnownMarketplaces`, `enabledPlugins` (preview, 2026-06-25); org-managed pin "can't be re-enabled, disabled, or repointed locally" | source types and match rules not stated in fetched pages | G2, G1 |
| Droid | managed `strictKnownMarketplaces` referenced; syntax not detailed | not stated | D |
| Cursor | admin-only team marketplaces (Enterprise), restrictable to Organization Groups | git imports only | U |
| Junie, Qoder | no admin policy stated | | J, Q |

## Q5 - Documented incidents and advisories (mutable-endpoint class)

- **polyfill.io** (2024): domain sold to Funnull in Feb 2024, malicious redirects by June; 490,000+ sites per cside (widely quoted 100,000 was a search cap). Mechanism: consumers loaded code from a URL whose controller changed; no content pin. [PF, secondary]
- **Plugin4Shell** (2026-09): pin verification missing in four agents; git sources. [P1]
- **Claude Code #37340** (opened 2026-03-22, closed "not planned", stale): official marketplace git repo auto-synced at startup, new plugins active with no notice; reporter asked for SHA pinning and diffs. Undocumented env var `CLAUDE_CODE_DISABLE_OFFICIAL_MARKETPLACE_AUTOINSTALL` exists (now in [CO]). [I1]
- **PromptArmor** (2025-10-16): third-party marketplaces on GitHub, scraped hourly by a registry site that "don't verify individual plugins"; installed plugin hooks bypass permissions. No vendor response documented. [I2]
- **Open VSX** (2025-10 token leak; 2026-01-30 GlassWorm via compromised publisher account, four established extensions, 22,000+ downloads; further waves March 2026): registry publish-token compromise, not URL hosting. [V1, V2]
- **Chrome extension ownership transfer** turning malicious (2026-03) [E]; Cyberhaven publisher compromise 2024-12 (search snippet). Update channel trusted the account/owner, not content.
- **Homebrew**: "A non-official tap is executable code, not plain metadata ... require explicit trust by default"; third-party taps unsupported and unvetted. Repojacking of tap owner names is described only in secondary search results; Homebrew's page does not address it. [H]
- **Dangling DNS to GitHub/GitLab Pages**: GitHub and GitLab both document the pre-verification takeover window (Q3). Search results cite an August 2026 wildcard-CNAME hijack (secondary, not fetched).
- Claude's own warning: Anthropic "cannot verify that they will work as intended or that they won't change." [CS]

## negative:

- No harness documents a marketplace-level digest or SHA pin for a URL-hosted catalog. `#ref`/`@sha` exist only for git marketplaces (Claude `#ref`; Droid `#ref`/`@sha`; Codex `--ref`).
- No source states whether Claude Code honours ETag / Cache-Control / conditional GET for URL marketplaces.
- Copilot CLI and Codex docs list no URL-marketplace add form; Codex `allowed_marketplaces` has no URL rule type.
- Copilot `strictKnownMarketplaces` match semantics, and Droid's syntax, are absent from fetched pages.
- Junie, Qoder, Cursor: no integrity, pinning, admin allowlist, or removed-entry behaviour documented for URL marketplaces.
- No incident or advisory found that concerns a plugin marketplace *served from Pages* specifically; the closest are dangling-DNS takeovers of Pages domains and mutable-URL script takeovers (polyfill.io).
- Plugin4Shell writeups do not say whether `archive` (sha256) or `url` marketplace paths were affected.

## leads:

- Measure first-hand (needs a permitted shell): `curl -sI` on a GitHub Pages and a GitLab Pages `marketplace.json` for `Cache-Control`, `ETag`, `Last-Modified`; then run `claude plugin marketplace update` behind a logging proxy to see whether `If-None-Match` is sent.
- Test Claude with an `archive` entry that omits `sha256`, change the zip, run `plugin update`: confirm the digest-derived version updates silently.
- Test `archive` + `sha256` swap in the catalog with the manifest `version` set, to confirm the "no update" inference.
- Read Copilot managed-settings docs and Droid `strictKnownMarketplaces` reference for source-type and matching rules; read the Codex marketplace-config page for `[marketplaces]` keys and revision handling.
- Fetch primary Plugin4Shell advisories from Anthropic and OpenAI release notes to confirm patched surfaces.
- GitLab: read repository-path-change docs for namespace-release timing; GitHub: confirm whether a deleted user/org name is claimable for `*.github.io` Pages URLs.
