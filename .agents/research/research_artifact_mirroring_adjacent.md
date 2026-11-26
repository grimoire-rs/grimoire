# Research: Distro and proxy practice for packaging upstream software you did not write

## Metadata

**Date:** 2026-11-23
**Domain:** packaging
**Triggered by:** pre-plan discussion `.agents/discussions/artifact-mirroring.md`, adjacent-fields lane (distributions, immutable proxies). Companion to `research_artifact_mirroring_prior_art.md`, which is not repeated here.
**Expires:** 2027-05-23

Scope: findings only, no recommendation. Every claim: URL, read date 2026-11-23, mark. `[page]` = I opened the page. `[search]` = search-result summary only, a lead. Where a fetch returned only part of a page, the gap is stated.

---

## Q1. Version derivation for upstream without releases

**Debian**
- Format `[epoch:]upstream_version[-debian_revision]`. Epoch is a small integer, default 0, and "should not be used to roll back a package; use `+really`". The revision is split at the last hyphen, a missing revision equals `0`, and by convention the revision restarts at `1` when upstream_version rises. A version with no hyphen is a native package. https://www.debian.org/doc/debian-policy/ch-controlfields.html [page]
- Ordering: epoch first, then a digit/non-digit alternating compare; `~` sorts before everything, even the end of a part. So `1.0~beta1` < `1.0`. https://www.debian.org/doc/debian-policy/ch-controlfields.html [page]
- Snapshot scheme: uscan's `Git-Pretty` default is `0.0~git%cd.%h`, with `Git-Date` default `%Y%m%d`. The `Matching-Pattern: HEAD` mode builds the version from the date and hash of HEAD. `Git-Pretty: describe` gives `git describe --tags` with `-` turned to `.`. The man page does not say what happens when tag matching finds no tags. https://manpages.debian.org/unstable/devscripts/debian-watch.5.en.html [page]
- Ordering against a later release: `0.0~git…` sorts below `0.0` and below any `0.x` or `1.x`, so a real release always wins without an epoch. The `~` rule above implies this; the watch page does not state it. (Inference from the policy text.)
- Repack suffix: `+dfsg` when non-free files are removed, `+ds` when files are removed for another reason, `+repack` when re-downloaded without a version change. `Dversion-Mangle` strips them before comparison. https://manpages.debian.org/unstable/devscripts/debian-watch.5.en.html [page]
- Packaging-only change: `debian_revision` (`-1`, `-2`), reset to 1 when upstream_version rises; `+nmu`, `+b`, `~bpoNuX` mark other kinds. https://www.debian.org/doc/debian-policy/ch-controlfields.html [page]

**Nix / nixpkgs**
- A commit with no assigned version: version "should be the latest upstream version preceding that commit, followed by `-unstable-`" and the commit date in `YYYY-MM-DD`. With no prior release the prefix is `0`. A version "must start with a digit". https://raw.githubusercontent.com/NixOS/nixpkgs/master/pkgs/README.md [page] (older name `unstable-YYYY-MM-DD` is not on this page; the current form is `0-unstable-YYYY-MM-DD`.)
- Sources: "When fetching from GitHub, always reference revisions by their full commit hash"; sha256 is the preferred hash. Same page [page].
- No separate packaging revision exists in the version string. A rebuild with changed packaging gets a new store path from the changed derivation; `nix-update --version=skip` updates hashes without a version change. https://github.com/Mic92/nix-update [page] (the "no revision field" part is inference.)
- `nix-update --version=branch` and `--version=branch=<name>` move an untagged package to the newest commit. https://github.com/Mic92/nix-update [page]

**Arch**
- No tags: `r$(git rev-list --count HEAD).$(git rev-parse --short HEAD)`, e.g. `r1142.a17a017`. With tags: `git describe --long` piped through sed, e.g. `2.0.r6.ga17a017`. The `r` delimiter exists because "without the delimiter monotonicity fails - `0.1.456 < 454`". The page does not mention epochs. https://cdn.jsdelivr.net/npm/arch-wiki-md-repo@0.10.84/wiki/_content/english/VCS%20package%20guidelines.md [page] (a mirror of the wiki, not the wiki itself; wiki.archlinux.org returned an Anubis block.)
- Packaging-only revision: `pkgrel` (not on the fetched page; general Arch knowledge, unverified here).
- VCS sources use `SKIP` for checksums because they are "not static". Same page [page].

**Go**
- Pseudo-version `vX.Y.Z-yyyymmddhhmmss-abcdefabcdef` (12-char revision id, UTC commit time). Three forms by base: no base `vX.0.0-…`; pre-release base `vX.Y.Z-pre.0.<ts>-<rev>`; release base `vX.Y.(Z+1)-0.<ts>-<rev>`. It "compares higher than its base version, but lower than the next tagged version". The go command checks the timestamp matches the commit and that the revision is an ancestor of a branch or tag. https://go.dev/ref/mod [page] (first 100000 of 215534 characters read)
- No separate packaging revision; the module is the source.

**conda-forge**
- Rule: "Packages should be built from tarballs using the `url` key, not from repositories directly by using e.g. `git_url`." https://conda-forge.org/docs/maintainer/adding_pkgs/ [page]
- Pinned commit with no tags: pin with `rev:` and the full commit hash, and use `0.0.0.YYYY.MM.DD` (date of the pinned commit) as the version. https://conda-forge.org/docs/maintainer/knowledge_base/ [page]
- Packaging-only change: bump the build number by 1; reset to `0` when the version changes. https://conda-forge.org/docs/maintainer/updating_pkgs/ [page]

**Comparison:** Debian, Arch, Go and conda-forge all encode a date or count so that a later real release sorts higher; Debian does it through `~`, Arch through the `r` delimiter, Go through the pre-release base, conda-forge through a `0.0.0` prefix. Only Debian, Arch (`pkgrel`) and conda-forge (build number) keep a packaging-revision field separate from the upstream version in the ordering. Go and Nix do not.

## Q2. One upstream repository into many packages

- **Debian:** the first stanza of `debian/control` is the source package, each later stanza is one binary package. A binary package's `Version` defaults to the source version; the `Source:` field can carry a different version in parentheses. So the per-unit version is the one source version unless the maintainer sets a binary version. https://www.debian.org/doc/debian-policy/ch-controlfields.html [page]
- **Nix:** one derivation produces several outputs, each in its own store path, "building from source always produces all the outputs". Outputs share one build and one version. https://raw.githubusercontent.com/NixOS/nixpkgs/master/doc/stdenv/multiple-output.chapter.md [page]
- **conda-forge:** multi-output recipes cover "shared source split into separate artifacts, different dependency sets, or alias names". The page lists pitfalls (build scripts apply only to the top-level package; name collisions between top-level and output names) and "doesn't describe a per-output version field". https://conda-forge.org/docs/maintainer/knowledge_base/ [page]
- **Go:** a repository with several `go.mod` files carries one tag prefix per module (`sub/dir/v1.2.3`). Not read on the pages I opened; lead below.
- Finding across the three read: all three give every output the one source version. None of the pages describes deriving an independent version per unit from a repo-level version.

## Q3. Update detection

- **Debian uscan:** polls from `debian/watch`. `Mode: git` with `Matching-Pattern: refs/tags/…` runs `git ls-remote` and joins matched groups with dots. `Matching-Pattern: HEAD` or `heads/<branch>` makes a date-and-hash version, and the repo must be cloned "even with `--no-download`", "a last resort". Exit status 1 means "No newer upstream versions were located". Silent on a repo with no tags and no HEAD pattern. https://manpages.debian.org/unstable/devscripts/uscan.1.en.html and https://manpages.debian.org/unstable/devscripts/debian-watch.5.en.html [page]
- **nix-update / r-ryantm:** `nix-update` defaults to the next stable version; `--version=branch` follows the default branch; "nixpkgs-update is optimized for mass-updates … nix-update is better suited for interactive usage". Whether r-ryantm handles untagged packages is not stated. https://github.com/Mic92/nix-update [page]
- **conda-forge autotick:** polls PyPI and GitHub ("monitors the release page or the tags"), stops when a feedstock has three or more open version-update PRs, supports a `bot.version_updates.exclude` list. Silent on commit-only sources. https://conda-forge.org/docs/maintainer/updating_pkgs/ [page]
- **Homebrew livecheck:** strategy `Git` fetches all tags and is preferred; `GithubLatest` and `GithubReleases` only when `Git` is not enough. It tries `stable`, `head`, `homepage` URLs in that order and returns an error if no version is found. `skip "reason"` exists. Silent on no-tag repos. https://docs.brew.sh/Brew-Livecheck [page]
- **Homebrew admission:** "Software without a stable release is difficult to reproduce, bottle and support and is not eligible for `homebrew/core`"; upstream must "identify the packaged version as stable and provide an immutable tag or release". https://docs.brew.sh/Acceptable-Formulae [page]
- **Repology:** labels `outdated`, `legacy`, `newest`, `devel`; `rolling` ("fetched from always latest snapshot or VCS master/trunk") is "not a subject for comparison"; `ignored` covers "snapshots"; `noscheme` marks projects whose repository versions are "basically random"; `unique` marks a version seen in one repository family only, "not really reliable". No time-based staleness threshold. https://repology.org/docs/about [page]
- **Arch:** a `-git` package builds whatever HEAD is; the talk page says maintainers should not bump `pkgver` by hand. https://wiki.archlinux.org/title/Talk:VCS_package_guidelines [search]
- Finding: for untagged upstream, the tag-based bots (conda-forge, Homebrew livecheck) have no documented behaviour. Debian (HEAD pattern), nix-update (`--version=branch`) and Arch (build HEAD) have a documented mode, each as an explicit opt-in per package.

## Q4. Recording origin

- **Debian copyright (DEP-5):** header fields `Format` (required), `Upstream-Name`, `Upstream-Contact`, `Source` ("Typically this would be a URL, but it might be a free-form explanation"; must say if upstream was modified to remove non-free parts). `Files` stanzas need `Files`, `Copyright`, `License`; "the last stanza that matches a particular file applies". https://www.debian.org/doc/packaging-manuals/copyright-format/1.0/ [page]
- **Debian control:** `Homepage` points "preferably" at the original source or upstream documentation; `Vcs-Git`/`Vcs-Browser` name the packaging repository (source stanza only), not the upstream one. https://www.debian.org/doc/debian-policy/ch-controlfields.html [page]
- **Nix:** `meta.license` "must be set and match the upstream license"; sources fetched by pinned fetchers, full commit hash for GitHub, `sha256` hash. The page has no dedicated `meta.homepage` rule. https://raw.githubusercontent.com/NixOS/nixpkgs/master/pkgs/README.md [page]
- **SLSA v1.0 requirements (retired page; v1.2 is current):** provenance "unambiguously identifies the output package by cryptographic digest"; L2 adds a signature from the build platform; L3 requires every field be generated or verified by the build platform. The page does not name the source repository as a required attested element, does not address repackagers, and gives "a system of multiple independent rebuilders" as one build-platform shape. So what a repackager can truthfully attest from this page: its own build step and the output digest, not the author's build. https://slsa.dev/spec/v1.0/requirements [page]. The v1.2 text, which may add a source track, was not read.
- **Chainguard:** SLSA v1 provenance for builds; sample shows buildtype `https://chainguard.dev/buildtypes/apkoaas/v1` and an `org.opencontainers.image.source` annotation pointing at Chainguard's own build definition; SBOM and other attestations are separate; signing via Sigstore with the signer identity pinned to a Chainguard workflow. Whether upstream commits or hashes are attested was not confirmed. https://edu.chainguard.dev/compliance/slsa/slsa-chainguard/ [search] (page redirects to https://www.chainguard.dev/docs/compliance/slsa, not opened)
- **Docker Hardened Images:** attestations "align with SLSA provenance", describe "how, when, and where the image was built", and are signed (cosign-compatible). No SLSA level named; source repo and relation to upstream not addressed. https://docs.docker.com/dhi/core-concepts/provenance/ [page]

## Q5. Author relations

- **Debian, removal:** the maintainer normally requests removal (`RM:` bug against `ftp.debian.org`, `ROM` if the requester is the maintainer). Others must give reasons and mention the maintainer's view. Licensing is named only as an exception to "not release critical". "Abandoned upstream" appears as an example reason. The wiki has no category for upstream requests. https://wiki.debian.org/ftpmaster_Removals [page], https://www.debian.org/doc/manuals/developers-reference/pkgs.html [page]
- **Debian, precedent:** a 2011 `imapsync` removal with title "RM: imapsync -- RoM; author doesn't want us to distribute his program". One thread, not policy. https://lists.alioth.debian.org/pipermail/pkg-perl-maintainers/2011-January/037970.html [search]
- **Debian, takeover:** "It is not OK to simply take over a package without assent of the current maintainer"; neglected packages go through the salvaging process; orphaning goes to `Debian QA Group`. The developers-reference text on how an upstream author becomes maintainer was not found; page read to 100000 of 135882 characters. https://www.debian.org/doc/manuals/developers-reference/pkgs.html [page]
- **Homebrew:** will "not usually" deprecate, disable or remove because upstream asks; technical breakage, policy violations, project needs weigh more. Lifecycle deprecate, disable, then automatic removal one year after the disable date. https://docs.brew.sh/Working-with-Homebrew-as-an-Upstream-Project [page, per prior scan]; lifecycle https://docs.brew.sh/Deprecating-Disabling-and-Removing [search]. No DMCA or takedown path found.
- **conda-forge:** "Anyone can submit a package, whether or not they maintain the upstream project"; informing upstream and inviting them to become maintainers is "not required but considered good practice". An abandoned feedstock is taken over with `@conda-forge-admin, please add user @username`, escalating to `conda-forge/core` after about a week; inactive maintainers are kept in the list. Archiving stops bot updates and keeps existing packages available (via an `admin-requests` file). Takedown and feedstock removal are not covered on the page. https://conda-forge.org/docs/maintainer/maintainer_faq/ and https://conda-forge.org/docs/maintainer/updating_pkgs/ [page]
- **Docker Official Images:** handover clause, per prior scan: "If you do not represent upstream and upstream becomes interested in maintaining the image, steps should be taken to ensure a smooth transition." https://github.com/docker-library/official-images [page]
- **Nix:** removal and upstream-request policy not found. Lead.

## Q6. Immutable-copy takedown

- **Go module proxy:** "We do not accept requests to remove cached modules or versions from proxy.golang.org." Deleting the source does not remove the module from the mirror. Exception: if no suitable licence is detected, only a temporary cached copy is kept and "may disappear"; modules also expire "if your LICENSE doesn't permit indefinite caching". The mirror "doesn't keep all modules forever". `sum.golang.org` is an append-only transparency log: "records cannot be modified or removed"; checksums remain after the module is gone; anomalies go in `sum.golang.org/errata`. Malicious modules: report to `security@golang.org`. https://proxy.golang.org/ [page]
- **Go retract:** the author publishes a higher version containing `retract` directives. Retracted versions are hidden from `@latest` and `go list -m -versions` but "should remain available … to ensure that builds that depend on them are not broken". https://go.dev/ref/mod [page]
- **Lockfile user after a Go retraction:** builds with the version in `go.sum` still work; the go command warns when upgrading or listing (the warning behaviour is not on the part I read). Only `@latest` and range queries skip the version.
- **npm unpublish:** within 72 hours, if no public package depends on it. After that, only if no dependents, under 300 downloads in the last week, and a single owner. A used name+version "you can never use it again"; full unpublish blocks new versions for 24 hours. Otherwise `npm deprecate`. Policy violations and copyright go through npm support and the DMCA policy. https://docs.npmjs.com/policies/unpublish [page]
- **cargo yank:** "does not delete any data"; "Existing lock files or direct downloads are not affected"; a new project without a lockfile cannot select the yanked version, and a dependent whose only matching version is yanked errors. https://doc.rust-lang.org/cargo/commands/cargo-yank.html [page]
- **crates.io deletion:** the policies page returned no text. Search results report that deletion is limited by conditions, owners may ask for an exception, and the team can delete crates that break policy; RFC 3660 proposed owner deletion under limited conditions (under 72 hours, or single owner with no reverse dependencies) and its adoption is unconfirmed. DMCA notices are handled by the Rust Foundation. https://rust-lang.github.io/rfcs/3660-crates-io-crate-deletions.html [search], https://rust-lang.github.io/rfcs/3463-crates-io-policy-update.html [search]
- Finding: Go and Cargo keep lockfile-pinned content reachable after a takedown request or yank. Go refuses removal requests outright, except for the no-licence case. npm allows deletion in a narrow window and blocks it once other packages depend on the name.

## Q7. Licence gates

- **Debian:** "Every package in main must comply with the DFSG"; `contrib` also DFSG-compliant but depends on non-free; non-compliant packages go in `non-free`. A program with no copyright notice is still copyrighted and "you may not do anything with it without permission"; a notice with no stated permissions allows nothing; packages whose notices do not permit even binary redistribution "must not be placed on the Debian FTP site and its mirrors at all". https://www.debian.org/doc/debian-policy/ch-archive.html [page]
- **Nix:** `meta.license` "must be set and match the upstream license"; "If there is no upstream license, `meta.license` should default to `lib.licenses.unfree`" and the maintainer should try to contact upstream. Unfree is blocked by default ("cannot be installed and doesn't show up in searches"), enabled by `allowUnfree`, `allowUnfreePredicate`, or `NIXPKGS_ALLOW_UNFREE=1`. Unfree is not built or cached by Nixpkgs CI. https://raw.githubusercontent.com/NixOS/nixpkgs/master/pkgs/README.md and https://raw.githubusercontent.com/NixOS/nixpkgs/master/doc/using/configuration.chapter.md [page]
- **conda-forge:** SPDX identifier recommended, case-sensitive. "Ensure that you have included a license file if your license requires one – most do." If the archive has none, add one to the recipe and notify upstream. The page does not say what to do with no licence at all. https://conda-forge.org/docs/maintainer/adding_pkgs/ [page]. Staged-recipes threads show maintainers adding a licence file from statements found in a README. https://github.com/conda-forge/staged-recipes/pull/15926 [search]
- **Homebrew:** `homebrew/core` formulae must be "open source under a licence compatible with the Debian Free Software Guidelines"; binary-only proprietary software goes in a cask; a formula with a non-open-source licence "should be removed". Unlicensed software not addressed on the page. https://docs.brew.sh/Acceptable-Formulae [page]; https://docs.brew.sh/Deprecating-Disabling-and-Removing [search]
- **Go proxy:** no suitable licence detected leads to a temporary cache only (see Q6). https://proxy.golang.org/ [page]
- Finding: for upstream with no licence file, Debian refuses, Nix files it as unfree (opt-in), Go caches temporarily, Homebrew and conda-forge have no written rule on the pages read. All five treat "licence present and permissive" as the precondition for plain public distribution.

---

## negative:

- Arch wiki and ftp-master REJECT-FAQ: `wiki.archlinux.org` blocked by Anubis; `ftp-master.debian.org/REJECT-FAQ.html` returned 404. Arch data comes from a wiki mirror.
- conda-forge: no documented policy for sources with no licence at all, for removing a feedstock at upstream's request, or for polling commit-only sources.
- Debian: no developers-reference text for an upstream author becoming maintainer, and no written policy for upstream-requested removal. The developers-reference page was read only to 100000 of 135882 characters.
- Nix: no found policy on upstream-requested removal or on packages with no tags and a bot.
- Homebrew: no DMCA path, no unlicensed-software rule on the pages read.
- crates.io: policies page returned no text; deletion rules are search summaries only.
- SLSA v1.2 (current) was not read; v1.0 page is retired. Whether it covers repackagers is unconfirmed.
- Go reference: pages beyond character 100000 (checksum database, GOPRIVATE) were not read.
- Chainguard docs: `docs.chainguard.dev` did not resolve; page content is search-only.
- Debian `0.0~git…` ordering against a later real release is inferred from the `~` rule; no page states it for snapshots.

## leads:

- Go multi-module repositories: sub-directory tag prefixes and how `go.mod` location sets per-module version (one tagged repo, many units). Closest analogue to Q2.
- Debian `uscan`/`gbp import-orig` snapshot practice: real examples of `0.0~git…+ds` in the archive and `Files-Excluded`. Shows packaging revision plus repack suffix together.
- conda-forge `admin-requests` and the `staged-recipes` licence rules: written terms for removal and for no-licence upstream.
- Go `GOPRIVATE`/`GONOSUMDB` and sumdb verification: how a client with a lockfile (`go.sum`) detects a changed copy of an already-seen version.
- SLSA v1.2 and in-toto: what a repackager can attest about content it did not build (source track, "provenance of a copy").
- Nixpkgs RFCs on packages with no maintainer or upstream disputes: removal and "maintainerless" handling.
- Docker Hardened Images and Chainguard provenance pages (opened, not search-only): scope of the claims about upstream source.
- crates.io policies page (live text) and RFC 3660 outcome: confirm owner-deletion rules.

## Sources

| Source | Type | Date read | Relevance |
|---|---|---|---|
| https://www.debian.org/doc/debian-policy/ch-controlfields.html | Policy | 2026-11-23 | Version format, epoch, ordering, Source/Binary, Homepage, Vcs |
| https://manpages.debian.org/unstable/devscripts/debian-watch.5.en.html | Man page | 2026-11-23 | Git snapshot default `0.0~git%cd.%h`, repack suffix |
| https://manpages.debian.org/unstable/devscripts/uscan.1.en.html | Man page | 2026-11-23 | uscan exit status, compression |
| https://www.debian.org/doc/packaging-manuals/copyright-format/1.0/ | Spec | 2026-11-23 | DEP-5 fields |
| https://www.debian.org/doc/debian-policy/ch-archive.html | Policy | 2026-11-23 | DFSG, non-free, no licence |
| https://wiki.debian.org/ftpmaster_Removals | Wiki | 2026-11-23 | Removal requests |
| https://www.debian.org/doc/manuals/developers-reference/pkgs.html | Manual | 2026-11-23 | Removal, orphaning, adoption (partial) |
| https://raw.githubusercontent.com/NixOS/nixpkgs/master/pkgs/README.md | Repo doc | 2026-11-23 | Nix version naming, licence, sourcing |
| https://raw.githubusercontent.com/NixOS/nixpkgs/master/doc/using/configuration.chapter.md | Repo doc | 2026-11-23 | allowUnfree |
| https://raw.githubusercontent.com/NixOS/nixpkgs/master/doc/stdenv/multiple-output.chapter.md | Repo doc | 2026-11-23 | Multiple outputs |
| https://github.com/Mic92/nix-update | Repo | 2026-11-23 | `--version=branch` |
| https://cdn.jsdelivr.net/npm/arch-wiki-md-repo@0.10.84/wiki/_content/english/VCS%20package%20guidelines.md | Wiki mirror | 2026-11-23 | Arch VCS pkgver |
| https://go.dev/ref/mod | Reference | 2026-11-23 | Pseudo-versions, retract (partial) |
| https://proxy.golang.org/ | Service doc | 2026-11-23 | No removal, sumdb, licence expiry |
| https://conda-forge.org/docs/maintainer/adding_pkgs/ | Docs | 2026-11-23 | Tarball rule, licence, maintainers |
| https://conda-forge.org/docs/maintainer/knowledge_base/ | Docs | 2026-11-23 | Multi-output, commit pin version |
| https://conda-forge.org/docs/maintainer/updating_pkgs/ | Docs | 2026-11-23 | Autotick bot, build number, archive |
| https://conda-forge.org/docs/maintainer/maintainer_faq/ | Docs | 2026-11-23 | Abandoned feedstock, upstream maintainers |
| https://docs.brew.sh/Brew-Livecheck | Docs | 2026-11-23 | livecheck strategies |
| https://docs.brew.sh/Acceptable-Formulae | Docs | 2026-11-23 | Stable release, licence rules |
| https://repology.org/docs/about | Docs | 2026-11-23 | Version statuses |
| https://docs.npmjs.com/policies/unpublish | Policy | 2026-11-23 | Unpublish rules |
| https://doc.rust-lang.org/cargo/commands/cargo-yank.html | Docs | 2026-11-23 | Yank semantics |
| https://slsa.dev/spec/v1.0/requirements | Spec (retired) | 2026-11-23 | Provenance scope |
| https://docs.docker.com/dhi/core-concepts/provenance/ | Docs | 2026-11-23 | DHI attestation claims |
| Search-only: Chainguard SLSA, crates.io RFCs 3463/3660, Homebrew deprecation page, conda-forge staged-recipes PR, imapsync RM thread, Arch talk page | mixed | 2026-11-23 | Leads; verify before citing |
