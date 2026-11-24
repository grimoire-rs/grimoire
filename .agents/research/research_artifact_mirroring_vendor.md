# Research: Vendor handling of third-party-listed and republished agent-config and extension content

## Metadata

**Date:** 2026-11-23
**Domain:** packaging
**Triggered by:** pre-plan discussion `.agents/discussions/artifact-mirroring.md`; follow-up to `research_artifact_mirroring_prior_art.md` (opens its `[search]` items as primary pages)
**Expires:** 2027-05-23

Scope: findings only, no recommendation. Every URL read 2026-11-23. Marks: `[page]` = I opened the page via WebFetch (extracts are model-summarised by the tool, so exact quotes are the tool's, not verbatim page copies); `[search]` = search summary only. Sources older than ~18 months are flagged `[OLD]`.

## Q1. Open VSX `open-vsx/publish-extensions`

- **Entry shape.** Minimal documented entry is `"<publisher>.<name>": { "repository": "https://github.com/<owner>/<repo>" }`; publisher and name come from the extension's `package.json`. https://github.com/EclipseFdn/open-vsx.org/wiki/Auto-Publishing-Extensions `[page]`
- **Fields in the live `extensions.json`:** `repository`, `location` (subdirectory), `prepublish`, `custom` (array of shell commands), `extensionFile`, `pythonVersion`, `timeout`, `target` (with `env`, `runsOn`), `universal`. https://raw.githubusercontent.com/open-vsx/publish-extensions/master/extensions.json `[page]`; option list per https://raw.githubusercontent.com/open-vsx/publish-extensions/master/DEVELOPMENT.md `[page]` (points to `extension-schema.json`).
- **There is no `version`, `ref` or `checkout` key.** Version comes from upstream `package.json`. Pins are written as shell inside `custom`, e.g. a `git checkout <sha>` guarded by `[ $VERSION = x.y.z ]`, or `git checkout origin/main` (a branch) with a "temporary" note. Five entries pin a commit this way. The updater "tries to find a commit to a build associated with the version using tags and commits around the last MS Marketplace updated date." extensions.json and DEVELOPMENT.md `[page]`
- **Build runs arbitrary repo code.** The entry's build steps (`custom`, `prepublish`, npm install) execute upstream code in the publishing workflow. Prior-art file records the 2025 token exposure from this. https://blogs.eclipse.org/post/mikaël-barbero/open-vsx-security-update-october-2025 (cited in prior scan)
- **Opt-out / takeover.** Author opt-out is a PR removing the entry from `extensions.json`; no other path documented. https://github.com/EclipseFdn/open-vsx.org/wiki/Auto-Publishing-Extensions `[page]`. Namespace takeover is by a public GitHub issue on `EclipseFdn/open-vsx.org`; auto-published entries are "removed from the list when a maintainer claims ownership". The wiki does not say what happens to already-published versions. https://github.com/eclipse/openvsx/wiki/Namespace-Access `[page]` `[OLD]` (edited 2025-03-18)
- **Verified flips on namespace ownership, not on content.** A version shows verified only if its namespace is verified and the publishing user is a member. Unverified shows a warning icon and banner. "Creating a namespace does not automatically assign you as verified owner." The `@open-vsx` account may publish without being a member ("exclusive privilege", to be used sparingly). Namespace-Access `[page]`; https://github.com/EclipseFdn/open-vsx.org/wiki/Publishing-Extensions `[page]` (label text: "That user account is not a verified publisher of the namespace.")
- **Licence precondition.** CONTRIBUTING: "We only accept extensions with an OSI-approved open source license here"; proprietary extensions "cannot be published through this repository". https://raw.githubusercontent.com/open-vsx/publish-extensions/master/CONTRIBUTING.md `[page]`. Wiki: non-author submitters must show the extension is open source; the Eclipse Committer Agreement must be signed before a PR merges. Auto-Publishing wiki `[page]`. Publishing wiki words it as "open source or otherwise permits redistribution" (for requesting auto-publish from the VS Marketplace). No SPDX field exists in the entry; the check is human at PR review (inference from the entry shape).
- **First-step guidance.** Open an issue with the owner first; this repo is "a temporary workaround" if the owner is unresponsive. CONTRIBUTING `[page]`.
- **Separate deny file.** `extension-control/extensions.json` holds malicious and deprecated extensions; its schema was not retrievable (README 404). Repo README `[page]`.

## Q2. Claude Code plugin marketplaces

- **Source shapes** (per entry): relative path, `github {repo, ref, sha}`, `url {url, ref, sha}`, `git-subdir {url, path, ref, sha}` (sparse checkout, partial clone), `npm {package, version, registry}`, `archive {url, sha256}`, `command {command, timeout, mode}`. https://code.claude.com/docs/en/plugins/marketplace-reference.md `[page]`
- **`ref` vs `sha`.** `ref` is a branch or tag, default branch if omitted. `sha` is a full 40-char lowercase commit; when both are set, `sha` is checked out. Install succeeds after the `ref` is deleted upstream "as long as the commit is still reachable"; some servers (AWS CodeCommit) need the ref to exist. marketplace-reference `[page]`
- **Version when omitted.** Order: manifest `version`, then entry `version`, then source-derived. `github`/`url`/`git-subdir`: commit SHA shortened to 12 chars (`git-subdir` adds a hash of the path). `archive`: 12 chars of SHA-256. `npm`: `unknown`. `command`: hash of produced files. https://code.claude.com/docs/en/plugins/loading.md `[page]`
- **Update behaviour.** An update happens only when the computed version changes. A manifest-pinned `version` freezes users even as upstream commits land; omitting `version` makes users track commits. Unpinned git sources float with the branch head. Auto-update is off by default for third-party marketplaces, on for Anthropic's official ones. loading.md and https://code.claude.com/docs/en/plugins/host-marketplace.md `[page]`
- **Copy at install.** Every non-local marketplace plugin is copied into `cache/<marketplace>/<plugin>/<version>/`; the catalog entry is the pointer, the cache is the copy. Old versions are removed 14 days after update. loading.md `[page]`
- **Third-party listing.** The docs frame a marketplace as "a group you choose" curating "your plugins"; there is no text on listing plugins the owner did not write and no ownership proof between entry and upstream repo. `author`, `homepage`, `license` display fields set on the entry override `plugin.json`; before install, only relative-path entries can be read for `plugin.json`, so other sources show only the entry's own claims. marketplace-reference `[page]`. Impersonation protection covers marketplace names only (reserved list, look-alike and non-ASCII names). marketplace-reference `[page]`
- **Removal.** No deprecation state. Owner removes the entry, maps the name to `null` in `renames`, optionally sets `forceRemoveDeletedPlugins` to uninstall from users. A marketplace-delisted plugin is recorded in `flagged-plugins.json`. host-marketplace and loading.md `[page]`
- **Licence.** `license` is a free display field; no check found. marketplace-reference `[page]`

## Q3. skills.sh and `npx skills`

- **Listing is telemetry-driven, no author step.** "Skills appear on the leaderboard automatically through anonymous telemetry when users run `npx skills add <owner/repo>`." https://skills.sh/docs/faq `[page]`. Telemetry off with `DISABLE_TELEMETRY=1` or `DO_NOT_TRACK=1` (also disables audit requests). https://github.com/vercel-labs/skills `[page]`
- **Identity.** GitHub `owner/repo` (plus skill name). https://skills.sh/docs `[page]`. Prior scan notes ranking ties to repo identity and a migration question (community.vercel.com, `[search]`).
- **Audits.** "Routine security audits" per the docs page; flagged-malicious hiding and the three scanners come from the Vercel changelog (https://vercel.com/changelog/automated-security-audits-now-available-for-skills-sh, cited in prior scan). The docs page itself does not cover hiding. https://skills.sh/docs `[page]`. `/docs/audits` returned 404.
- **Removal / takedown.** Not documented. FAQ: skills "are maintained by their respective authors"; for problems, open an issue in the skill's GitHub repo. Security reports to security.vercel.com. skills.sh/docs and /docs/faq `[page]`
- **Pointer plus local copy plus lock.** The CLI copies files into `.agents/skills/<name>`; `skills-lock.json` v2 records `computedHash` per skill and, for git sources, the `commit`; installs fetch that commit, not latest on `ref`; `npx skills update` moves the commit forward. https://github.com/vercel-labs/skills `[page]`
- **Licence.** CLI repo is MIT; no licence handling for listed skills found. https://github.com/vercel-labs/skills `[page]`

## Q4. Official MCP registry

- **Namespace proof.** GitHub (`io.github.<user>/*`, org namespace needs org Owner role) or domain (DNS TXT at apex, or `/.well-known/mcp-registry-auth`, keypair). https://modelcontextprotocol.io/registry/authentication `[page]`
- **Non-author publishing.** The namespace rule means a third party can publish only under their own namespace (`io.github.mirrorer/...`). Package ownership is also checked per type: npm `mcpName` in `package.json`, PyPI/NuGet/Cargo `mcp-name:` in the README, OCI annotation `io.modelcontextprotocol.server.name`. The name must equal the `server.json` name. So pointing at someone else's package only works if that package already carries the mirrorer's name. MCPB is an exception: only a `fileSha256` field and "mcp" in the URL, and the registry does not validate the hash (clients do). https://modelcontextprotocol.io/registry/package-types `[page]` (implication stated is mine).
- **Labelling.** No "mirror" or "third-party" label found; the namespace string itself carries the publisher identity. https://modelcontextprotocol.io/registry/about `[page]`
- **Pointer-only.** "The MCP Registry hosts metadata that points to those packages." Version metadata is immutable once published. about + https://modelcontextprotocol.io/registry/faq `[page]`
- **Moderation.** Removal grounds: illegal content (incl. copyright violations), malware, spam, non-functioning servers. Not grounds: low quality, vulnerabilities, duplicates. Process: `status` set to `deleted`, metadata stays in the API; extreme cases may be overwritten. No removal-request process described; appeals via a GitHub issue. https://modelcontextprotocol.io/registry/moderation-policy (fetched as the repo mirror `docs/modelcontextprotocol-io/moderation-policy.mdx`) `[page]`
- **Author-side delisting.** `mcp-publisher status --status deleted`; deleted hidden by default, restorable, "never permanently removed". FAQ `[page]`
- **Licence.** Supports open- and closed-source servers if installable or reachable publicly; no licence field requirement found. about `[page]`

## Q5. Aggregators

- **PulseMCP.** Submit page: "We are not accepting new MCP server or client submissions right now", "not making changes to existing listings"; says publish to the Official MCP Registry and "we will pick it up automatically". No claim or removal text. https://www.pulsemcp.com/submit `[page]`. ETL-from-registry claim is a third-party blog: https://pub.towardsai.net/mcp-registries-in-mid-2026-one-upstream-won-2ce541b92036 `[search]`. Registry moderation page says aggregators "may" drop deleted servers (they are not obliged). moderation-policy `[page]`
- **Cursor directory (`cursor/community-plugins`).** Submitter signs in with GitHub or Google, pastes a GitHub repo URL at `cursor.directory/plugins/new`; components auto-detected (`rules/*.mdc`, `.mcp.json`). The README does not require the submitter to own the repo. Each submission gets an AI security scan (verdict safe, suspicious, malicious) and admin approval; admins can unpublish or permanently block; a "verified" badge exists with verification requests. Whether content is stored or fetched live is not stated (scan uses "a fresh download of its repo"). https://raw.githubusercontent.com/cursor/community-plugins/main/README.md `[page]`
- **Cursor removal.** Forum posts: self-serve delete/unpublish unreliable, staff remove by hand. https://forum.cursor.com/t/unable-to-remove-published-plugins-from-cursor-directory-account/164577 `[search]`
- **Cursor Marketplace Publisher Terms** (the Cursor Marketplace, not necessarily cursor.directory): publisher retains title, grants Cursor a sublicensable licence while listed, indemnifies for third-party IP; only permissive licences (MIT, BSD, Apache 2.0), copyleft "may not be included"; removal by written notice to the legal address; existing users keep installed copies; Cursor may remove at any time. https://cursor.com/marketplace-publisher-terms `[page]`
- **Smithery.** Docs fetches returned 404 (`smithery.ai/docs/build/publish`, `www.smithery.ai/docs/build/publish.md`). Search summary only: GitHub-connected deploy, a "Get verified" heading, a `servers.repo.delete` action that removes the repo connection, no documented listing delete. https://smithery.ai/docs/build/publish `[search]`
- **Glama.** Claim flow via `/.well-known/glama.json` on the server's domain with a matching account email. https://glama.ai/mcp/remote-servers/ai.smithery/smithery-ai-github `[search]`

## Q6. Copy or pointer, and the cost when upstream moves

| System | Listing stores | Install-time source | Upstream deleted / force-pushed |
|---|---|---|---|
| Open VSX auto-publish | COPY: built `.vsix` published to open-vsx.org | the published copy | Copy persists. Next nightly build fails or skips if repo gone. Moved tag breaks a `custom` pin silently (inference) |
| Claude marketplace entry | POINTER (`source` object) | fetched at install, then COPY in plugin cache keyed by version | `sha`-pinned: installs while commit reachable, even if `ref` deleted; force-push that orphans the commit breaks fresh installs (inference from "reachable"). Unpinned: follows head, so version changes on every push. Existing cache keeps working |
| `npx skills` / skills.sh | POINTER (`owner/repo` identity) | fetched, COPIED to `.agents/skills`; lock records `commit` + `computedHash` | Lock fetches the recorded commit; `update` moves it. Mutable-URL hijack research on skills.sh at https://arxiv.org/abs/2603.16572 `[search]` |
| Official MCP registry | POINTER (metadata to npm, PyPI, OCI, MCPB URL) | whatever the package registry serves | Metadata versions immutable; code lives elsewhere; `deleted` status is soft |
| Cursor directory | POINTER implied (repo URL); storage unclear | not stated | not stated |
| Smithery | unverified | unverified | unverified |

Cost pattern from the pages above: copy-based listings keep working but keep serving content after upstream removal (Open VSX; Go proxy case in prior scan); pointer-based listings break or float unless a content pin (`sha`, `commit`, `sha256`, digest) exists. Only archive and git-SHA pins verify content; MCP registry does not validate its MCPB hash.

## Q7. Licence handling

- **Allowlist or required:** Open VSX auto-publish repo: OSI-approved only (CONTRIBUTING `[page]`). Cursor Marketplace terms: permissive only (MIT, BSD, Apache 2.0), copyleft excluded (`[page]`).
- **Display field, not checked:** Claude marketplace `license` field (`[page]`).
- **None found:** official MCP registry (requires public availability only), skills.sh, PulseMCP, cursor.directory README, Smithery (docs unreachable).
- **No SPDX requirement found in any system read.**

## negative:

- Smithery docs unreachable (404 on two URLs); claim, removal, copy-vs-pointer unverified.
- skills.sh: no documented author claim, takedown, or hide process; `/docs/audits` 404.
- PulseMCP: no claim or removal text; submissions paused.
- cursor.directory: no documented claim flow, no stated ownership check on submission, no licence text.
- Open VSX: no `ref`/`version`/`checkout` fields; no machine-readable licence field; what happens to already-published versions after namespace claim not documented.
- Claude marketplace docs say nothing on third-party-authored listings or on proof that an entry owner is the upstream author.
- No system read has a documented "author takes over a mirrored entry and keeps its history" flow other than Open VSX's namespace claim.
- Open VSX `extension-control` schema not retrieved.

## leads:

- Open VSX `extension-schema.json`: confirm the full field list and whether any field is licence-related.
- Open VSX namespace-claim outcomes: how many auto-published entries were removed after a claim and what happened to their published versions (issues on `EclipseFdn/open-vsx.org`).
- MCP registry `mcpName`-style ownership proof as a model for an author-consent marker inside the upstream repo.
- skills.sh audit mechanics and hide process: `skills.sh/docs` index pages, Vercel security contact flow.
- Claude directory submission terms (`claude.com/docs/directory/publish`): the author-submits model, as contrast to marketplace-owner listing.
- Smithery and Glama docs via another route (CLI source, API reference at `smithery.ai/docs/api-reference/servers/publish-a-server.md`).
- `skills-lock.json` v2 format as a lock-with-commit-and-hash precedent for pointer entries.

## Sources

| Source | Mark | Covers |
|---|---|---|
| https://github.com/EclipseFdn/open-vsx.org/wiki/Auto-Publishing-Extensions | page | entry shape, opt-out, licence for non-authors |
| https://raw.githubusercontent.com/open-vsx/publish-extensions/master/extensions.json | page | live fields, pins via `custom` |
| https://raw.githubusercontent.com/open-vsx/publish-extensions/master/DEVELOPMENT.md | page | options, version discovery |
| https://raw.githubusercontent.com/open-vsx/publish-extensions/master/CONTRIBUTING.md | page | OSI licence rule, issue-first |
| https://github.com/eclipse/openvsx/wiki/Namespace-Access | page, OLD | verified, claim, `@open-vsx` |
| https://github.com/EclipseFdn/open-vsx.org/wiki/Publishing-Extensions | page | unverified label, publisher agreement |
| https://code.claude.com/docs/en/plugins/marketplace-reference.md | page | source shapes, ref/sha, display fields |
| https://code.claude.com/docs/en/plugins/loading.md | page | version computation, cache, update |
| https://code.claude.com/docs/en/plugins/host-marketplace.md | page | update, hold, rename, remove |
| https://code.claude.com/docs/en/plugin-marketplaces | page | source choice overview |
| https://skills.sh/docs, https://skills.sh/docs/faq | page | telemetry listing, identity, no takedown |
| https://github.com/vercel-labs/skills | page | lock v2, commit pin, telemetry flags |
| https://modelcontextprotocol.io/registry/authentication | page | namespaces |
| https://modelcontextprotocol.io/registry/package-types | page | per-type ownership proof |
| https://modelcontextprotocol.io/registry/about | page | pointer model, aggregators |
| https://modelcontextprotocol.io/registry/faq | page | delete, immutability |
| moderation-policy.mdx in modelcontextprotocol/registry | page | removal grounds |
| https://www.pulsemcp.com/submit | page | submissions paused |
| https://raw.githubusercontent.com/cursor/community-plugins/main/README.md | page | cursor.directory flow |
| https://cursor.com/marketplace-publisher-terms | page | licence and removal terms |
| smithery.ai docs, glama.ai, forum.cursor.com, towardsai, arxiv 2603.16572 | search | leads only |
