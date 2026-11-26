# Research: Repository split prior art (shared base, GUI over CLI, plugin repos, history extraction, Rust GUI)

<!--
Evidence only. No recommendation. Built from public web sources fetched on
2026-11-25. Several fetches returned summaries from a small model, not raw
pages; where a claim rests on a search summary only, the text says so.
-->

## Metadata

**Date:** 2026-11-25
**Domain:** devops | ci-cd | packaging | cli
**Triggered by:** Plan to split a Rust CLI project into a core repository, a `tool-<name>` PATH-dispatched plugin repository and a GUI repository that each share the core repository's base.
**Expires:** 2027-05-25

## Direct Answer

No single question has one settled answer in the sources. The evidence per question is below. Gaps are named in each section and again under "Not verified".

## Key Findings

### Q1. Sharing a repository base across repositories

**GitHub template repositories.**
- A repository created from a template starts with one commit and unrelated history. The page does not say that later template changes reach existing repositories. Source: [GitHub Docs, creating a repository from a template](https://docs.github.com/en/repositories/creating-and-managing-repositories/creating-a-repository-from-a-template) (fetched 2026-11-25).
- Drift is the documented complaint. A GitHub community thread asks how to sync template changes ([community discussion 23528](https://github.com/orgs/community/discussions/23528), seen in search results only, not read). A DevRev engineering write-up on template repositories exists ([Medium, DevRev](https://medium.com/devrev/learnings-from-using-github-templates-at-devrev-7f254b846b86)); the fetch returned 403, so its content is unverified. A search summary says it and an OSDU ADR describe forks falling behind the template.
- Third-party sync tools exist that open pull requests from a template: [actions-template-sync](https://github.com/AndreasAugustin/actions-template-sync) (supports private templates and a `.templatesyncignore` file, per search summary) and [Repobot file sync](https://github.com/apps/repobot-file-sync). The OSDU project recorded its own sync decisions in [ADR-011](https://azure.github.io/osdu-spi/adr/011-configuration-driven-template-sync) and [ADR-012](https://azure.github.io/osdu-spi/adr/012-template-update-propagation-strategy) (titles only seen; contents not read).

**Template engines with update support.**
- Copier's own comparison page says Copier supports template updates, only for Git-hosted templates, using Git tags to compute diffs between versions. It says cookiecutter has no updates, "possible through Cruft". Source: [Copier comparisons](https://copier.readthedocs.io/en/stable/comparisons/). This is the Copier project describing itself; treat as self-interested.
- A search summary of independent sources says Copier runs a three-way merge between the old rendered output, the project and the new template, and cruft offers `cruft check` for CI drift detection. One user report in a university GitLab issue said cruft was not useful for updating projects, which led that team to test Copier ([issue 89](https://gitlab.developers.cam.ac.uk/uis/devops/webapp-boilerplate/-/issues/89), seen in search results, not read). Treat as anecdote.
- Real project using Copier: Diamond Light Source's Python template. Its page says Copier pairs rendering with "an update mechanism to apply diffs of the updated template to the project", and that cookiecutter made updates hard so changes were copied by hand. It also says forking a skeleton repository and merging upstream blurred contribution history and gave messy merges when upstream edits touched text containing the project name. Source: [Diamond Light Source, why use a template](https://diamondlightsource.github.io/python-copier-template/main/explanations/why-use-template.html). The page lists no downsides of Copier itself.
- A Copier discussion titled about migration of older template versions says its migration support is not a complete fix ([copier-org discussion 1637](https://github.com/orgs/copier-org/discussions/1637), search summary only, not read).
- Cruft: repository forks and maintenance status were not verified. Several forks appear in search results (gmsantos, projectsyn, cruft org), which may indicate unclear upstream ownership; this is an inference, not a verified fact.

**GitHub reusable workflows and composite actions.**
- Reusable workflows chain up to ten levels and a single file may call at most 50 unique reusable workflows. Caller `env` does not pass to the called workflow. A private caller can use public and private reusable workflows, a public caller only public ones, and a private called workflow needs an access policy allowing the caller. A non-SHA reference reruns "all jobs" at the reference's current version. The caller is billed and the called repository's hosted runners are unavailable. Source: [GitHub Docs, reusing workflow configurations](https://docs.github.com/en/actions/reference/workflows-and-actions/reusing-workflow-configurations) (fetched 2026-11-25).
- Composite actions run as steps inside the caller's job and show as one step in the log; secrets must be passed as inputs. Source: search summary of GitHub docs and a [2026 blog comparison](https://steve-kaschimer.github.io/posts/2026-03-13-github-actions-reusable-workflows-vs-composite-actions), not read in full. Older sources say four nesting levels; current docs say ten.
- These share workflow logic only. They do not share lint configuration, task-runner files, agent configuration, licence or contribution files.

**Organisation `.github` repository.**
- Default community health files (CONTRIBUTING, CODE_OF_CONDUCT, SECURITY, SUPPORT, issue and pull request templates, FUNDING) apply to repositories of the same account that lack their own copy. The repository must be public. Licence files cannot be defaulted. A repository's own valid issue template config replaces the whole default folder. Defaults do not appear in clones. Source: [GitHub Docs, default community health file](https://docs.github.com/en/communities/setting-up-your-project-for-healthy-contributions/creating-a-default-community-health-file) (fetched 2026-11-25). The page does not say whether workflows are inherited; reusable workflows are the documented route. Workflow starter templates in the same repository were not verified.

**Shared configuration presets.**
- Renovate documents shareable presets: a repository `renovate-config` is auto-detected in the same organisation, a `default.json` there becomes the organisation default, presets are pinned by git tag, later presets win and local settings override. Npm-based presets are deprecated. Source: [Renovate, shareable config presets](https://docs.renovatebot.com/config-presets/) (search summary; page listed, not read in full). This is a mature pull model for one tool's config. Equivalents for rustfmt, clippy, deny and Cargo lints were not researched; rustfmt and clippy have no `extends` mechanism as far as this research checked, which is unverified.

**Remote includes in task runners.**
- Task (go-task) remote Taskfiles graduated from experiment to generally available after nearly three years; the feature is on by default from v3.53.0; the post says no substantial API changes were made recently. Source: [Taskfile blog, remote taskfiles](https://taskfile.dev/blog/remote-taskfiles) (fetched 2026-11-25; the page shows no explicit date, and files the post under 2026; the header showed v3.54.0). The post does not cover trust, checksums or offline use. The schema page documents a `checksum` property on includes ([Task schema reference](https://taskfile.dev/docs/reference/schema)). Trusted hosts, `--offline`, cache expiry and git URL pinning were not verified because the dedicated page was not read.

**Git submodules and subtrees.**
- Submodules: anecdotal sources only. A tmatesoft post calls them "very inconvenient and unsafe" ([tmatesoft blog](https://blog.tmatesoft.com/how-we-stopped-worrying-about-submodules-and-focused-on-code), title seen in search, not read). A Debian developer essay argues they are always the wrong solution ([diziet](https://diziet.dreamwidth.org/14666.html), not read). Counterpoint from a comment thread: simple for dependencies. All low authority.
- Subtrees: the `git subtree` manual page returned 404 and was not read. No evidence collected on subtree used for sharing a base. Unverified.
- Both mechanisms place shared files at fixed paths inside each repository, so dotfiles at the repository root (workflows, `.claude/`) cannot live in a submodule without a copy step. This is an inference, not a sourced claim.

**One monorepo with independent release trains.**
- No primary source was gathered. The related facts found: Cargo supports workspaces, and Cargo's own docs say linking the `cargo` crate as a library is discouraged because its API is unstable ([Cargo reference, external tools](https://doc.rust-lang.org/cargo/reference/external-tools.html)). Evidence on release-train tooling (for example release-plz, cargo-dist, per-crate tags) and on projects that regretted or chose a monorepo was not gathered. Unverified.

### Q2. How a GUI consumes a CLI core

- **Shell out and parse structured output.** Cargo documents `cargo metadata` as a stable, versioned format and tells callers to pass `--format-version`; `--message-format=json` messages are described but no explicit stability promise is stated. Cargo also discourages linking its crate as a library because the API is unstable. Source: [Cargo reference, external tools](https://doc.rust-lang.org/cargo/reference/external-tools.html) (fetched 2026-11-25). Dugite, used by GitHub Desktop, exposes git through the same command line interface: an `exec(['status'], path)` call returning exit code, stdout and stderr, which suggests child processes; the README does not say so explicitly. Source: [dugite README](https://github.com/desktop/dugite). Whether Fork, GitKraken or other Git GUIs link libgit2 was not verified.
- **Jujutsu GUIs split.** The Jujutsu community tools page lists GG (a GUI), JayJay (macOS, Rust and SwiftUI), LightJJ (browser UI), many TUIs and editor plugins ([jj community tools](https://github.com/jj-vcs/jj/blob/main/docs/community_tools.md)). GG uses Tauri (a `Tauri.toml` exists), reads settings via `jj config`, and also offers `gg web`; the README does not say whether it links `jj-lib` ([gg repository](https://github.com/gulbanana/gg)). A search summary said JayJay wraps `jj-lib` directly and that jj-cz moved from shelling out to `jj-lib`; this is a search summary, not verified against the repositories. The `jj-lib` docs page makes no stability statement and calls Jujutsu "an experimental version control system" ([docs.rs jj-lib](https://docs.rs/jj-lib/latest/jj_lib/)).
- **Plugin or extension protocol with a metadata handshake.** Docker CLI plugins must be named `docker-<name>` and answer a `docker-cli-plugin-metadata` subcommand returning JSON: `SchemaVersion` (mandatory, "0.1.0", later any major below 2), `Vendor` (mandatory), `Version`, `ShortDescription`, `URL`, `Hidden`. Source: [docker/cli metadata.go](https://pkg.go.dev/github.com/docker/cli/cli-plugins/metadata) (search summary of source; not read line by line). Docker Desktop extensions declare the Extension API version they need and Desktop limits installs to matching ones; image tags follow semver. Source: [Docker extensions architecture and distribution docs](https://docs.docker.com/extensions/extensions-sdk/architecture/) (search summary).
- **Daemon and API.** Podman Desktop talks to the Docker socket API for Docker resources and calls the Podman CLI for Podman resources (search summary of [Red Hat article](https://developers.redhat.com/articles/2024/05/07/manage-containers-podman-desktop), not read). The same results say DDEV moved from direct CLI calls to Docker's client library for compatibility with alternative runtimes. Both are third-party descriptions.
- **GitHub CLI extensions** are repositories named `gh-<name>` holding an executable of the same name; arguments are forwarded; the page states no I/O or exit-code contract; extensions cannot override core commands; they are "not verified, signed, or endorsed by GitHub". Source: [gh extension manual](https://cli.github.com/manual/gh_extension). A Go helper library, go-gh, is offered to extension authors ([go-gh](https://github.com/cli/go-gh)).
- **kubectl and cargo contracts.** kubectl plugins are `kubectl-<name>` on PATH, longest-match resolution, environment inherited, arguments passed as-is, no plugin-specific variables, cannot override built-ins; Krew index plugins are not audited. Source: [Kubernetes docs, kubectl plugins](https://kubernetes.io/docs/tasks/extend-kubectl/kubectl-plugins/). Cargo runs `cargo-<command> <command> [args]`, checks `$CARGO_HOME/bin` before PATH, and gives the `CARGO` environment variable for calling back. Source: Cargo reference above.
- **MCP or language-server style protocols for a GUI.** No source was gathered for a CLI exposing such a mode to a GUI. Unverified.
- Docker Desktop's GUI-to-engine internals and docker/compose version pinning against the CLI were not found.

### Q3. Plugin binaries in their own repository

- The documented plugin contracts (gh, kubectl, cargo, docker) name the discovery convention and argument passing. gh checks for a newer extension version every 24 hours and can notify; none of the pages read describe a compatibility range between plugin and host, apart from Docker's `SchemaVersion` handshake and Docker Desktop's Extension API version. Sources as in Q2.
- No source was found for a plugin test suite run against a matrix of host versions or a host built from a git revision. The search found Cargo's own CI guide recommending a toolchain matrix plus an allowed-to-fail "latest dependencies" job ([Cargo CI guide](https://doc.rust-lang.org/stable/cargo/guide/continuous-integration.html), search summary), cargo-hack (feature and version testing, [repository](https://github.com/taiki-e/cargo-hack)) and cargo-minimal-versions; none was verified as testing against multiple Cargo versions. gh's own docs suggest `gh extension install .` for local testing only (search summary of [gh docs](https://docs.github.com/en/github-cli/github-cli/creating-github-cli-extensions)). Evidence for the three options asked about (pinned release, version matrix, git revision) is therefore absent from this research, not negative.

### Q4. Extracting a directory into a new repository with history

- `git filter-repo` manual: no option handles signatures. It states that rewritten commits get new hashes, so "signatures on commits and tags cannot continue to work and instead are just removed"; signed tags become annotated tags. It aborts unless run in a fresh clone (use `--no-local` for local clones); `--path` keeps listed paths and does not follow renames, so old paths must be listed; `--subdirectory-filter` makes a directory the root; `--to-subdirectory-filter` does the reverse; `commit-map` records old to new hashes; abbreviated hashes in messages are rewritten unless `--preserve-commit-hashes`. Source: [git-filter-repo manual](https://man.archlinux.org/man/git-filter-repo.1.en.raw) (fetched 2026-11-25; GitHub-hosted manual returned 503).
- Sign-off trailers are ordinary message text. The manual excerpts read do not say they are touched, apart from the hash rewrite in messages. Not tested here; the search summary's inference that they survive is unverified.
- Signed-commit handling is in progress upstream. `git fast-import --signed-commits=<mode>` offers `verbatim` (default), `strip`, `strip-if-invalid`, `sign-if-invalid[=<keyid>]`, `abort-if-invalid` and warn variants. Source: [git-fast-import documentation](https://git-scm.com/docs/git-fast-import) (fetched 2026-11-25; the page does not say which Git version added each mode). The filter-repo author's issue describes options (A) to (E), says fast-export now exports signatures, and says he will update filter-repo once fast-import support lands; no dates on the page and no confirmation that filter-repo now exposes a flag. Source: [GitLab issue 510](https://gitlab.com/gitlab-org/git/-/issues/510).
- A monorepo-splitting article says signatures are always removed when splitting ([Embedded Artistry, 2023](https://embeddedartistry.com/blog/2023/01/26/dealing-with-signed-commits-when-creating-and-splitting-a-monorepo/), search summary).
- `git subtree split`: the manual returned 404, so no first-hand evidence. It also rewrites commits, which implies new hashes and lost signatures; this is an inference, not sourced.
- No other alternatives (for example `git filter-branch` deprecation text, `git-subrepo`) were verified beyond filter-branch's own docs saying signed tag signatures are stripped (search summary).

### Q5. Rust GUI options for a small CLI companion

Facts only. Version numbers and release dates were not shown on the pages fetched, so currency is unverified.

- **Tauri 2.** WebView based. Windows needs Microsoft Edge WebView2 (preinstalled on Windows 10 1803 and later); Linux builds use the `webkit2gtk-4.1` packages; macOS 10.15 and later. Distribution formats listed: deb, rpm, AppImage, Flatpak, Snap, AUR, DMG and app bundle, Windows installer and store; signing is "required on most platforms" and macOS direct distribution needs notarization. Sources: [Tauri prerequisites](https://v2.tauri.app/start/prerequisites/) and [Tauri distribute](https://v2.tauri.app/distribute/). A search summary quotes a 3.2 MB hello world bundle against Electron in one July 2026 test and a 2023 benchmark of about 5 MB; both from low authority blogs. Accessibility depends on the platform webview; no first-party Tauri statement was found. GG, a Jujutsu GUI, uses Tauri.
- **egui and eframe.** Pre-1.0; README says new releases will have breaking changes; used for professional apps such as Rerun Viewer. AccessKit integration is optional, enabled by default in eframe, implemented for Windows and macOS natively; other platforms get an experimental built-in screen reader on web. Immediate mode: full layout per frame, non-Latin text needs your own font, no native look. No binary size figure on the page. MIT or Apache-2.0. Source: [egui README](https://github.com/emilk/egui) (fetched 2026-11-25). Search summary of a 2023 benchmark gave egui about 18 MB and iced about 17 MB; low authority and dated.
- **iced.** README: "currently experimental software". The accessibility issue [iced #552](https://github.com/iced-rs/iced/issues/552) was open at the last comment in Nov to Dec 2025, with System76's COSMIC fork carrying AccessKit integration and a pull request revival discussed since June 2024. Showcase apps: Halloy, Icebreaker. Source: [iced README](https://github.com/iced-rs/iced).
- **Slint.** Three licence choices: royalty-free, GPLv3, commercial. Platforms: Windows, macOS, Linux, Android, iOS, embedded, web. Renderers: femtovg, Skia, software. Accessibility: a search summary reports desktop screen reader support since Slint 0.2.5 (2022) and an AccessKit adapter in the winit backend; the current accessibility guide was not read and per-platform completeness is unverified. The README gives design goals, no footprint numbers. Sources: [Slint README](https://github.com/slint-ui/slint), [Slint 0.2.5 release](https://slint.dev/blog/slint-0.2.5-released), [winit backend accesskit.rs](https://docs.rs/crate/i-slint-backend-winit/1.9.2/source/accesskit.rs).
- **Dioxus.** Default desktop renderer is a webview; WGPU and Freya (Skia) renderers are marked experimental, as is hot-patching. README claims desktop binaries under 3 MB and under 5 MB for desktop and mobile without supporting data. README names versions 0.6 (stable) and 0.7 docs. Source: [Dioxus README](https://github.com/DioxusLabs/dioxus). The native renderer Blitz is "in a beta state", lists AccessKit as a goal and depends on it in `blitz-shell`; git Dioxus depends on 0.8.0-alpha.3. Source: [Blitz README](https://github.com/DioxusLabs/blitz).
- **GPUI.** "Still pre-1.0"; "breaking changes between versions"; needs latest stable Rust; Windows (Win32, DirectWrite), Linux (wayland or x11 feature flags), macOS (Metal). Learning material is the Zed source and Discord. The page only links an accessibility topic; its content was not read. Source: [GPUI README](https://github.com/zed-industries/zed/tree/main/crates/gpui). Whether it is published on crates.io was not verified.
- **AccessKit** (shared dependency of several toolkits): adapters for Windows (UI Automation), macOS, Unix (AT-SPI), Android and iOS at "rough feature parity"; no rich text or hypertext; a web adapter is planned. Source: [AccessKit README](https://github.com/AccessKit/accesskit).
- Are We GUI Yet lists Iced, egui, Slint, Tauri, GTK 4, Dioxus, Xilem, Freya, Floem, Vizia, Relm4, GPUI, Makepad, Kas (tagged accessible) and others, with no accessibility column otherwise. Source: [areweguiyet.com](https://areweguiyet.com/).

## Not verified

- Contents of the DevRev template write-up and OSDU ADRs; any maintainer text on regretting submodules or template repositories.
- Copier and cruft failure reports beyond the one anecdote; cruft maintenance status.
- Monorepo with independent release trains (no source). Subtree manual. Starter workflow templates.
- Task remote taskfile trust and cache details.
- Whether Git GUIs link libgit2; how GG talks to jj; Docker Desktop internals.
- Plugin test matrix practice (no source found).
- Whether `git filter-repo` now exposes a signature flag; Git version that added `--signed-commits`; trailer behaviour under filter-repo.
- Slint current accessibility guide; Tauri and Dioxus webview accessibility; GPUI accessibility; all binary sizes except vendor claims and dated blog numbers; current versions of every GUI crate.

## Sources

| Source | Type | Date | Relevance |
|--------|------|------|-----------|
| https://docs.github.com/en/repositories/creating-and-managing-repositories/creating-a-repository-from-a-template | Docs | read 2026-11-25 | Template repo behaviour |
| https://docs.github.com/en/actions/reference/workflows-and-actions/reusing-workflow-configurations | Docs | read 2026-11-25 | Reusable workflow limits |
| https://docs.github.com/en/communities/setting-up-your-project-for-healthy-contributions/creating-a-default-community-health-file | Docs | read 2026-11-25 | Org `.github` defaults |
| https://copier.readthedocs.io/en/stable/comparisons/ | Docs | read 2026-11-25 | Copier, cookiecutter, cruft |
| https://diamondlightsource.github.io/python-copier-template/main/explanations/why-use-template.html | Project docs | read 2026-11-25 | Real Copier user and reasons |
| https://docs.renovatebot.com/config-presets/ | Docs | search summary | Shared presets model |
| https://taskfile.dev/blog/remote-taskfiles | Maintainer post | 2026, no exact date | Remote Taskfiles stable |
| https://doc.rust-lang.org/cargo/reference/external-tools.html | Docs | read 2026-11-25 | Cargo subcommands, metadata stability |
| https://kubernetes.io/docs/tasks/extend-kubectl/kubectl-plugins/ | Docs | read 2026-11-25 | kubectl plugin contract |
| https://cli.github.com/manual/gh_extension | Docs | read 2026-11-25 | gh extension contract |
| https://pkg.go.dev/github.com/docker/cli/cli-plugins/metadata | Source docs | search summary | Docker plugin metadata |
| https://docs.docker.com/extensions/extensions-sdk/architecture/ | Docs | search summary | Docker Desktop extension API versioning |
| https://github.com/jj-vcs/jj/blob/main/docs/community_tools.md | Docs | read 2026-11-25 | Jujutsu GUIs |
| https://github.com/gulbanana/gg | Repo | read 2026-11-25 | GG on Tauri |
| https://docs.rs/jj-lib/latest/jj_lib/ | Docs | read 2026-11-25 | No stability statement |
| https://github.com/desktop/dugite | Repo | read 2026-11-25 | Git via command line |
| https://man.archlinux.org/man/git-filter-repo.1.en.raw | Manual | read 2026-11-25 | Signature removal, flags |
| https://git-scm.com/docs/git-fast-import | Docs | read 2026-11-25 | `--signed-commits` modes |
| https://gitlab.com/gitlab-org/git/-/issues/510 | Issue | undated | filter-repo signing plans |
| https://v2.tauri.app/start/prerequisites/ | Docs | read 2026-11-25 | Tauri webview dependencies |
| https://v2.tauri.app/distribute/ | Docs | read 2026-11-25 | Tauri packaging, signing |
| https://github.com/emilk/egui | Repo | read 2026-11-25 | egui maturity, AccessKit |
| https://github.com/iced-rs/iced/issues/552 | Issue | to Dec 2025 | iced accessibility status |
| https://github.com/slint-ui/slint | Repo | read 2026-11-25 | Slint licences, platforms |
| https://github.com/DioxusLabs/dioxus | Repo | read 2026-11-25 | Dioxus renderers |
| https://github.com/DioxusLabs/blitz | Repo | read 2026-11-25 | Blitz beta |
| https://github.com/zed-industries/zed/tree/main/crates/gpui | Repo | read 2026-11-25 | GPUI status |
| https://github.com/AccessKit/accesskit | Repo | read 2026-11-25 | AccessKit adapters |
| https://areweguiyet.com/ | Directory | read 2026-11-25 | Framework list |
