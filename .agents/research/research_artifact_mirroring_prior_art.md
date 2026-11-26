# Research: Prior art for mirroring artifacts into a package ecosystem

## Metadata

**Date:** 2026-11-23
**Domain:** packaging
**Triggered by:** pre-plan discussion `.agents/discussions/artifact-mirroring.md` (meaning of "mirroring" not yet pinned: (A) third-party ingest and repackaging, (B) registry-to-registry replication)
**Expires:** 2027-05-23

Scope: findings only. No recommendation. All URLs read on 2026-11-23 unless marked `[search]`. `[search]` means the claim comes from a web-search result summary that I did not open as a page; treat it as a lead, not a verified fact. Sources older than about 18 months are flagged `[OLD]`.

---

## (A) Third-party ingest and republish

### Axis 1: Provenance and trust

- **Open VSX marks auto-published extensions as unverified.** A version is "verified" only when its namespace is verified and its publishing user is a member of the namespace. The privileged `@open-vsx` account publishes extensions that maintainers have not, "even if it is not a member of its namespace". The wiki lists this as a cause of the warning icon: "published by the privileged @open-vsx account although the namespace was owned by someone else". https://github.com/eclipse/openvsx/wiki/Namespace-Access (page last edited 2025-03-18 [OLD, ~20 months])
- **Claude Code marketplaces: the trust unit is the marketplace owner, not the plugin author.** An entry can point at any repo (`github`, `url`, `git-subdir`, `npm`, `archive`, `command`). The `author` and `homepage` display fields are set by whoever writes the entry (the entry's value wins over `plugin.json`). Nothing proves the entry owner is the plugin's author. Only reserved-name and impersonation rules protect the official marketplace names. https://code.claude.com/docs/en/plugins/marketplace-reference.md
- **Claude Code: only the `archive` source has a content hash.** `sha256` is optional on `archive`, and validate warns without it. Git sources can pin `sha`. https://code.claude.com/docs/en/plugins/marketplace-reference.md
- **skills.sh: third-party scanners, no author attestation.** Audits by Gen, Socket and Snyk, shown on each skill page. Skills flagged malicious are hidden from the leaderboard and search. From `skills@1.4.0` the CLI shows risk levels before install. The page says nothing on how skills get listed or whether authors opt in. https://vercel.com/changelog/automated-security-audits-now-available-for-skills-sh (2026-02-17)
- **MCP registry: provenance = namespace proof, not package proof.** Names are `io.github.<user|org>/*` (GitHub OAuth or Actions OIDC) or reverse-DNS (DNS TXT or `/.well-known/mcp-registry-auth`, keypair-based). Org namespace needs org Owner role, "so ordinary membership cannot publish or overwrite". A third party cannot publish under the author's namespace. Third parties publish under their own. The registry is "in preview". https://modelcontextprotocol.io/registry/authentication
- **Flathub: verified = maintained by the developer or a party the developer approved.** Third-party submissions are "not eligible for verification". Verification is a domain token at `/.well-known/org.flathub.VerifiedApps.txt` or a login provider (login-provider route closed to new `com.github.*`/`com.gitlab.*` IDs). https://docs.flathub.org/docs/for-app-authors/verification and https://docs.flathub.org/docs/for-users/verification (read as search result `[search]`; pages not opened)
- **Artifact Hub indexes without copying and separates "verified publisher" from "ownership claim".** Both are driven by an `artifacthub-repo.yml` file inside the indexed repo (for OCI, pushed under tag `artifacthub.io`). Claim is automated, "doesn't require contacting current owners", matched by email. https://artifacthub.io/docs/topics/repositories/ `[search]`
- **Chainguard (rebuild from source, republish).** Signs via Sigstore, ships SPDX SBOM and SLSA provenance as attestations, with certificate identity pinned to Chainguard's own workflow. Upstream signatures are not carried; Chainguard becomes the attestor of the rebuild. https://edu.chainguard.dev/chainguard/containers/how-to-use/retrieve-image-sboms/ `[search]`, https://edu.chainguard.dev/compliance/slsa/slsa-chainguard/ `[search]`
- **Docker Official Images: README mentions no image signing or attestation.** It covers only verifying upstream downloads in the Dockerfile. https://github.com/docker-library/official-images (read 2026-11-23). Docker's Hardened Images were not researched.

### Axis 2: Naming, namespace, handover, squatting

- **Open VSX: no owner means unverified, anyone with a token can publish.** Namespaces closed to non-members on 2020-12-17. Ownership is claimed by a public GitHub issue. Most auto-published extensions are in unverified namespaces and are "removed from the list when a maintainer claims ownership". https://github.com/eclipse/openvsx/wiki/Namespace-Access [OLD]
- **Open VSX squatting and evil-twin incidents.** 150 squatted names (Pluto Security, 2025-07). 77 packages republishing real extension names and namespaces from non-owners, 2026-07-26 to 2026-08-01, removed by 2026-08-03. Root cause per reporting: Open VSX and Microsoft's marketplace keep separate publisher-ownership systems, so a Microsoft namespace owner may not hold the same namespace on Open VSX. https://thehackernews.com/2026/08/open-vsx-removes-77-malicious-evil-twin.html (blocked 403; facts via `[search]` result summaries of Hacker News, BleepingComputer, TechRepublic), https://www.bleepingcomputer.com/news/security/77-open-vsx-extensions-found-harvesting-developer-info/ `[search]`
- **Eclipse said (January 2026) it was building impersonation detection.** Whether live during the August campaign is unconfirmed. https://thehackernews.com/2026/08/open-vsx-removes-77-malicious-evil-twin.html `[search]`
- **MCP registry avoids the collision by construction.** The mirror's name embeds the mirrorer's identity (`io.github.mirrorer/foo`), never the author's. No handover mechanism is documented on the page I read. https://modelcontextprotocol.io/registry/authentication
- **Docker Official Images: explicit handover clause.** "If you do not represent upstream and upstream becomes interested in maintaining the image, steps should be taken to ensure a smooth transition." Each image carries a mandatory `Maintainers` field. https://github.com/docker-library/official-images
- **conda-forge: the feedstock maintainer team is the unit.** Joining is a PR adding a GitHub ID to `recipe-maintainers`. If current maintainers are silent for a week, the core team can merge it. No special upstream-author path found. https://conda-forge.org/docs/maintainer/adding_pkgs `[search]`, https://conda-forge.org/community/joining-the-team/ `[search]`
- **Claude Code: marketplace-name collision is per user.** "A user can't have two marketplaces with the same name registered". Plugin id is `plugin@marketplace`, so the same plugin name can exist in several marketplaces. Reserved list blocks official-name impersonation, including non-ASCII names and look-alike spellings. https://code.claude.com/docs/en/plugins/marketplace-reference.md

### Axis 3: Version derivation (no release, no per-unit version)

- **Claude Code: the commit SHA is the version when `version` is omitted.** If `version` is set in the entry or `plugin.json` it pins, and `plugin.json` wins over the entry. Plugin entries take `ref` (branch/tag) and `sha` (40-char); `sha` wins over `ref`. `git-subdir` checks out one subdirectory of a monorepo, which is the "one skill in a repo of skills" case. https://code.claude.com/docs/en/plugins/marketplace-reference.md; omitted-version-means-SHA statement via `[search]` of https://code.claude.com/docs/en/plugin-marketplaces
- **Claude Code `command` source (mode `copy`): version = hash of copied files.** Link mode: version from the printed path and top-level entries. https://code.claude.com/docs/en/plugins/marketplace-reference.md
- **Go module proxy: pseudo-versions `v0.0.0-<timestamp>-<commit>`** are the commit-to-version mapping. Retraction does not cover pseudo-versions derived from a retracted release. Proxy keeps copies after source deletion (2026 arXiv measurement). https://golang.org/issue/24031 `[search]`, https://arxiv.org/pdf/2606.26291 `[search]`
- **Open VSX auto-publish: version comes from the upstream extension's `package.json`.** Details of `extensions.json` fields (ref/version pinning) were not retrievable from the repo README. https://github.com/open-vsx/publish-extensions (README only, fields not shown). Lead below.
- **Docker Official Images: tags listed in the library file are the supported set.** Removed tags are not removed from Docker Hub. https://github.com/docker-library/official-images

### Axis 4: Freshness

- **Bot-driven bumps are the norm for packagers.**
  - conda-forge: `regro-cf-autotick-bot` polls PyPI/GitHub/etc. and opens PRs. It stops opening PRs if more than 3 of its bump PRs are open. Pre-release judgement uses PEP 440, not the GitHub label. https://conda-forge.org/docs/maintainer/updating_pkgs `[search]`
  - Homebrew: autobump by BrewTestBot. Excluded if `deprecate!`/`disable!`/livecheck `skip`/`no_autobump!`. Disabled formulae are auto-removed one year after the disable date. https://docs.brew.sh/Autobump `[search]`
  - nixpkgs: `r-ryantm` runs `nixpkgs-update` and tags maintainers. Per-package opt-out comment exists. https://wiki.nixos.org/wiki/R-ryantm `[search]`
- **Open VSX auto-publish is a nightly job.** Its privileged token was exposed through `npm install` on untrusted extension code in the nightly workflow (reported 2025-05-04, sandboxed 2025-06-24, closed 2025-10-21). https://blogs.eclipse.org/post/mikaël-barbero/open-vsx-security-update-october-2025 (read; covers 2025-10 token leaks and planned scanning), https://securityaffairs.com/179398/hacking/taking-over-millions-of-developers-exploiting-an-open-vsx-registry-flaw.html `[search]`
- **Claude Code: unpinned git sources float.** Without `sha`, a branch ref moves on every refresh. Docs show pinning the marketplace itself by SHA (`ref` = full SHA, v2.1.324+, refused for reserved names). https://code.claude.com/docs/en/plugins/marketplace-reference.md
- **skills.sh mutable-URL hijack.** Research found 121 skills across 7 repos susceptible to hijack through mutable GitHub URLs. https://arxiv.org/abs/2603.16572 `[search]`
- **Takedown does not propagate.** Skills removed from the primary marketplace "continued to propagate through downstream registries and aggregators". Same source, `[search]` summary. Go proxy shows the same persistence.

### Axis 5: Licensing, consent, takedown

- **Homebrew: will not usually remove software at upstream's request.** Technical breakage, policy violations or project needs weigh more. https://docs.brew.sh/Working-with-Homebrew-as-an-Upstream-Project
- **Open VSX: opt-out is claiming the namespace.** The auto-publish list entry is removed on claim. Wiki says the privilege "should be used sparingly". https://github.com/eclipse/openvsx/wiki/Namespace-Access [OLD]
- **Docker Official Images: contributor must commit to maintain.** README shows no license check on submissions. https://github.com/docker-library/official-images
- **winget: Microsoft reserves the right to refuse any submission.** Pipeline per a July 2026 third-party post: schema, hash, malware scan, sandbox install, human moderator. No rule found on whether the submitter must be the software's publisher. https://learn.microsoft.com/windows/package-manager/package/repository `[search]`, https://msendpointmgr.com/2026/07/30/is-winget-enterprise-ready/ `[search]`
- **Flathub states that unverified listings may be third-party-maintained** and that the user-facing badge distinguishes them. https://docs.flathub.org/docs/for-users/verification `[search]`
- **AI skills: no source found on licence handling for scraped or re-listed skills** (negative). Related incident data: ClawHub malicious skills 341 then 824 after rescan (Palo Alto Networks, Koi). https://www.paloaltonetworks.com/blog/?p=373131 `[search]`
- **Anthropic directory: submission is by the author.** Each plugin folder needs its own submission; each version is rechecked; MCP connectors are listed as "Community" by default; terms are the Software Directory Terms and Policy. https://claude.com/docs/plugins/submit.md `[search]`, https://mixed-news.com/en/anyone-on-a-paid-claude-plan-can-now-submit-a-plugin-and-every-submission-gets-scanned/ `[search]` (opened 2026-09-25 per search summary)

### Axis 6: Discovery load

- **skills.sh grew 2,179 to 40,285 skills between 2026-01-16 and 2026-02-05** per one analysis; Vercel later cites 60,000+ audited. The leaderboard hides flagged skills and orders by install stats. https://arxiv.org/abs/2602.08004 `[search]`, https://vercel.com/changelog/automated-security-audits-now-available-for-skills-sh
- **Aggregators crawl GitHub for anything shaped like a skill** and republish it. https://airia.com/blog/skills-are-the-new-supply-chain-the-marketplace-is-not-ready/ `[search]`
- **A migration of a skill repo on skills.sh raised the question whether install stats reset.** Ranking is tied to the source repo identity. https://community.vercel.com/t/will-migrating-agent-skills-to-a-new-repository-on-skills-sh-reset-install-stats/35844.md `[search]`
- **winget: moderation was introduced in 2021 after a spike of duplicate and bad submissions.** https://www.theregister.com/2021/06/02/microsoft_introduces_moderation_for_winget/ `[search]` [OLD]
- **PulseMCP showed a banner that new submissions and listing changes were paused (2026-10-31 snapshot).** https://www.apideck.com/blog/mcp-server-directories `[search]`
- **Countermeasures seen:** per-skill public audit plus hide-on-flag (skills.sh); mandatory maintainers and "commitment to maintain" (Docker Official Images); human moderator (winget); verified badge separate from listing (Flathub, Artifact Hub, Open VSX); a freeze on submissions (PulseMCP).

---

## (B) Replication between registries

### Axis 1: Provenance across copy

- **Digest is the anchor.** A signature over a source digest will not verify against a different destination digest. Red Hat reports describe mirroring that changed digests (manifest conversion). https://oneuptime.com/blog/post/2026-08-11-cosign-signature-disappears-after-image-mirroring/markdown `[search]` (vendor blog, 2026-08), https://access.redhat.com/node/6778441 `[search]`
- **A manifest-only copy drops signatures, SBOMs, attestations.** The blog warns exit code 0 does not prove the supply-chain metadata arrived. Same oneuptime post `[search]`.
- **Cosign 3 stores signatures as OCI 1.1 referrers by default; older cosign uses digest-derived `.sig`/`.att`/`.sbom` tags.** A mirror must handle both. Same post `[search]`.
- **Harbor replication (to at least 2.15.x): copies manifest and layers byte-for-byte, does not carry OCI 1.1 referrers.** Reported: Cosign v3 signatures missed, legacy `.sig` tags copied, `oras copy --recursive` works against the same registries. https://github.com/goharbor/harbor/issues/23210 `[search]` (issue URL via search result mirror), https://trivoallan.github.io/houba/how-to/migrate-from-replication `[search]`. Not re-verified against Harbor release notes.
- **Harbor 2025 blog says replication keeps "attachments" (signatures, SBOMs, attestations) in sync** but does not explain the referrer mechanism. https://goharbor.io/blog/harbor-as-universal-oci-hub/ (2025-07-08, read). This conflicts with the issue above; version difference unresolved.
- **Flux Mirror (2026-08-12): `includeReferrers: true` copies SBOMs, signatures, provenance as OCI 1.1 referrers; a `verify` block enforces Cosign keyless identity (issuer plus subject) before copy; `minAge` measured against Rekor.** Failed tags are not copied. https://fluxcd.io/blog/2026/08/flux-mirror/
- **Zot on-demand sync is documented with cosign verification** and a key-management endpoint. https://docsearch.algolia.com/mcp/docs/repo/project-zot/zot `[search]`, thin source.
- **Tool flags:**
  - `regctl image copy --referrers` (plus `--digest-tags` for tag-style signatures). https://github.com/regclient/regclient `[search]`
  - `oras cp --recursive` documented as copying referrers (oras.land page 404 on direct fetch; via `[search]` summary).
  - `skopeo sync`: `--all`, `--preserve-digests` (fails if the digest cannot be preserved), `--keep-going`, `--remove-signatures` (implies signatures copied by default); referrer/sigstore behaviour unconfirmed. https://www.mankier.com/1/skopeo-sync `[search]`
  - `cosign copy --only=sig,att,sbom`: default changed between versions; one source reports a v2.2.1 regression, so pass `--only` explicitly. https://sources.debian.org/src/cosign/2.6.2-1/doc/cosign_copy.md (404 on fetch) `[search]`
  - `crane copy` retains digest; referrer handling unconfirmed. https://sources.debian.org/src/go-containerregistry/0.20.1%2Bds1-1/cmd/crane/doc/crane_copy.md `[search]`

### Axis 7: References, lockfiles, rewrite, pull-through vs copy

- **Host is part of the reference in OCI.** Consumers reach a mirror by rewriting at the runtime layer, or by changing the reference.
  - containerd `hosts.toml`: a `[host."https://mirror"]` entry redirects pulls and keeps the repository path (original registry passed as `ns` query). Hosts tried in order; `_default` falls back to upstream unless the mirror is set as `server`. Capabilities `pull` (low trust) vs `resolve` (name to digest, "trusted") exist: public mirror gets `pull` only. https://containerd.io/docs/2.2/hosts/
  - `registries.conf`: `mirror-by-digest-only = true` per `[[registry.mirror]]` restricts the mirror to digest pulls. https://docs.redhat.com/en/documentation/red_hat_build_of_microshift/4.16/html/getting_ready_to_install_microshift/microshift-deploy-with-mirror-registry `[search]`
- **Digest-pinned references survive any mirror; tag references need the mirror to resolve tags** (the containerd `resolve` capability). Inferred from the two sources above.
- **Flux does not rewrite references.** After Flux Mirror runs, consumers are repointed by hand (`OCIRepository`, `HelmRelease` shown with the new `oci://` URL). https://fluxcd.io/blog/2026/08/flux-mirror/ . `.spec.ref.digest` pins to a digest. https://fluxcd.io/docs/components/source/ocirepositories/ `[search]`
- **Pull-through cache vs explicit copy.**
  - Harbor proxy cache: forwards referrer API calls upstream when the option is on (container-registry.com's Harbor-based docs, 2.16; upstream Harbor behaviour not confirmed). https://container-registry.com/docs/2.16/administration-manual/replication/proxy-cache-projects/ `[search]`
  - Harbor replication, `regsync`, `skopeo sync`, `oras cp`: explicit copy, scheduled or one-shot. `regsync` can also run in check-only mode for stale images. https://github.com/regclient/regclient `[search]`
  - Zot on-demand sync is pull-through behaviour with local persistence. Same Zot source `[search]`.
- **Air-gapped:** guides describe stage, transfer, push; internal CA is the common failure. Mostly vendor blogs. https://oneuptime.com/blog/post/2026-03-05-oci-artifacts-air-gapped-flux/markdown `[search]`
- **Go module proxy is the reference case for a transparent proxy plus checksum DB** (immutable once seen) for (A)-style content. See Axis 3.

---

## negative:

- No source found that states how Smithery, Cursor directory, or skills.sh treat a listing of a skill whose author did not opt in, or how an author gets one removed. https://www.smithery.ai/docs/build/publish.md `[search]` covers publishing only.
- No source found on licence checks for aggregator-republished skills.
- Open VSX `publish-extensions` README does not describe `extensions.json` fields, opt-out, or licence rules. They may live in CONTRIBUTING.md (not read).
- winget: no rule found on submitter versus publisher identity.
- JSR/npm layer: nothing found on scope claim, name disputes, or third-party republish. JSR scope = personal or org name, claimed at first publish. https://jsr.io/docs/introduction `[search]`
- Docker Official Images: no evidence on signing or attestation in the README; Docker Hardened Images not researched.
- `skopeo sync`, `crane copy`: referrer and signature behaviour not confirmed from primary docs.
- Harbor replication and referrers: sources conflict (2025 blog says attachments replicate; 2025/26 issue says OCI 1.1 referrers are missed up to 2.15.x).
- `claude-plugins-community` marketplace name appears in the Claude Code reserved list ("community marketplace names") but I found no page describing what it is.
- Many (B) claims rest on vendor blogs (oneuptime.com, 2026-08) rather than tool docs.

## leads:

- Open VSX `publish-extensions` `extensions.json` and CONTRIBUTING: how pins, forks, removal and the verified-namespace flip work. This is the closest analogue to (A) bulk ingest by a non-author.
- Docker Hardened Images and Chainguard: what is carried when a republisher rebuilds (upstream signature dropped, own SLSA provenance added); useful to define what "mirror of X" attests.
- OCI Distribution spec 1.1 referrers API and fallback tag schema: decides whether a mirror can be expressed as digest-preserving copy plus referrers, and how registries without the API behave.
- Sigstore: cosign/Notation verification policies after copy (identity pinning to the original signer vs to the mirror); Flux Mirror `verify` block is a worked example.
- Go sumdb / proxy retraction and takedown semantics: how an immutable transparent mirror handles author takedown requests.
- Homebrew/Debian/Nix policies on upstream-requested removal and on upstream becoming the maintainer, for an opt-out design.
- Claude Code plugin loading doc (`/docs/en/plugins/loading.md`, "How Claude Code computes the version") and `host-marketplace.md` "Keep users up to date": exact version and update semantics for SHA-pinned vs floating sources.
- Mutable-URL hijack and downstream-propagation research on skill ecosystems (arXiv 2603.16572, 2605.28588): the threat model for unpinned ingest.
- MCP registry: delisting, moderation and takedown policy (not found on the auth page).

## Sources

| Source | Type | Date read | Relevance |
|---|---|---|---|
| https://github.com/eclipse/openvsx/wiki/Namespace-Access | Wiki | 2026-11-23 (edited 2025-03-18) | Open VSX namespace, verified, `@open-vsx` auto-publish |
| https://blogs.eclipse.org/post/mikaël-barbero/open-vsx-security-update-october-2025 | Blog | 2026-11-23 (2025-10-27) | Token leak, scanning plans |
| https://github.com/open-vsx/publish-extensions | Repo | 2026-11-23 | `extensions.json` auto-publish list |
| https://code.claude.com/docs/en/plugins/marketplace-reference.md | Docs | 2026-11-23 | Sources, ref/sha, reserved names, versions |
| https://code.claude.com/docs/en/plugin-marketplaces | Docs | 2026-11-23 | Marketplace creation, source choices |
| https://vercel.com/changelog/automated-security-audits-now-available-for-skills-sh | Changelog | 2026-11-23 (2026-02-17) | skills.sh audits |
| https://modelcontextprotocol.io/registry/authentication | Docs | 2026-11-23 | MCP registry namespace proof |
| https://github.com/docker-library/official-images | Repo | 2026-11-23 | Maintainers, handover, tags |
| https://docs.brew.sh/Working-with-Homebrew-as-an-Upstream-Project | Docs | 2026-11-23 | Upstream removal requests |
| https://containerd.io/docs/2.2/hosts/ | Docs | 2026-11-23 | Mirror host rewrite, capabilities |
| https://fluxcd.io/blog/2026/08/flux-mirror/ | Blog | 2026-11-23 (2026-08-12) | Referrer copy, verify, no rewrite |
| https://goharbor.io/blog/harbor-as-universal-oci-hub/ | Blog | 2026-11-23 (2025-07-08) | Harbor replication of attachments |
| Search-result-only items (marked `[search]` above): conda-forge, Flathub, Artifact Hub, nixpkgs, Homebrew autobump, winget, JSR, Go issues, Harbor issue, skopeo/cosign/crane/regctl/oras, Chainguard, Open VSX incident coverage, skills security papers | mixed | 2026-11-23 | Leads; verify before citing in an ADR |
