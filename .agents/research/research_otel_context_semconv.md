# Research: OTEL context attributes — vcs, cicd, host, user semconv and privacy

<!--
Technology Landscape Research
Filename: .agents/research/research_otel_context_semconv.md
Owner: Researcher (hex research phase)
Handoff to: Architect (/hex-architect), /hex-plan
Related Skills: hex-architect, hex-plan

Purpose: Persist tech landscape findings to inform ADRs, plans, design decisions.
Artifacts decay — check dates before trusting findings.
-->

## Metadata

**Date:** 2026-10-22
**Domain:** observability | security | cli
**Triggered by:** `.agents/discussions/otel-usage-telemetry.md` ([grimoire-rs/grimoire#206](https://github.com/grimoire-rs/grimoire/issues/206)). Owner question: what context does the record carry, and does grim auto-detect a git repository? Lane: context attributes and their privacy (web research). Extends, does not redo, `research_otel_codebase_recon.md`, `research_otel_cli_prior_art.md` and `research_otel_community_threads.md`.
**Expires:** 2027-04-22 (semconv ships a release about monthly: v1.41.0 28 Apr, v1.42.0 12 Jun, v1.43.0 3 Jul, v1.44.0 4 Aug; `opentelemetry-resource-detectors` shipped 0.11.0, 0.12.0, 0.13.0 between May and Sep 2026)

Conventions: **[>18mo]** marks a source older than about 2025-04-22. "Summarizer" marks a claim read through the WebFetch summarizing model (its quotes are extractions, not byte-exact fetches). "Two routes" means the claim was read on two independent pages or files. "Carried" means taken from the earlier research files, not re-read. "Inference" means my reading, not a sourced statement. Local file:line citations were read this session.

## Direct Answer

No recommendation (lane brief). Evidence only, per question.

**What context do comparable tools attach, and where does it come from?**

1. **The Rust SDK's default Resource carries no host, OS, process or VCS data.** `Resource::builder()` runs three detectors: `service.name` (from `OTEL_SERVICE_NAME`, else `OTEL_RESOURCE_ATTRIBUTES`, else `unknown_service:<exe file name>`), `telemetry.sdk.{name,language,version}`, and every key in `OTEL_RESOURCE_ATTRIBUTES`. It is the default for the trace provider (`opentelemetry_sdk-0.33.1/src/resource/mod.rs:62-70`, `src/resource/env.rs:75-107`, `src/trace/config.rs:36`).
2. **`opentelemetry-resource-detectors` (0.13.0, "beta") collects more than the semconv sensitivity guidance allows by default.** The process detector emits the full argv (`process.command_args`) with no redaction. The host detector shells out to `hostname` for `host.name`, and reads the raw `/etc/machine-id` for `host.id`. None of it is in the SDK, "due to their lack of stability" (README).
3. **Semconv `vcs.*` and `cicd.*` are Release Candidate** (promoted in v1.43.0); `user.*` and `host.*` are Development; `process.*` is Release Candidate (v1.44.0). The registry carries no sensitivity wording for `vcs.*`, `cicd.*`, `host.name` or `process.owner`. It does for `enduser.id` ("sensitive (PII)"), `url.full` (credentials MUST NOT appear) and `process.command_args` ("SHOULD NOT be collected by default unless there is sanitization").
4. **One shipping precedent exists for auto-detected `vcs.*` over OTLP to an operator-owned collector: Claude Code.** Off by default (`OTEL_METRICS_INCLUDE_REPOSITORY`), plaintext not hashed, lowercased, credentials/query/fragment stripped, derived from the `origin` remote once per session, omitted when there is no origin or the only enclosing repository is the home directory.
5. **grim already auto-detects a git repository, in a different command.** `grim build/release/publish` shell out to `git` and read `remote.origin.url`, with a unit-tested credential stripper; under the default `Auto` mode the remote URL and commit author are deliberately withheld from published annotations (`src/oci/git_provenance.rs:4-39, 182-217, 228-231, 265-271, 363-421`).

**Which privacy risks has the evidence shown?**

- Credentials inside remote URLs are real: GitLab documents `CI_REPOSITORY_URL` as `https://gitlab-ci-token:$CI_JOB_TOKEN@host/...`, and Bitrise began stripping tokens from stored repository URLs in July 2026. Two shipping normalizers have gaps (see Finding 8).
- Unsalted hashes of low-entropy inputs are treated as reversible by the OTel project, the FTC and the .NET incident; per-machine salts avoid that but cannot be joined across machines.
- Branch names, git emails and clone URIs are sent in plaintext by at least one operator-facing tool (Moderne); hashed by another (Alchemy).

## Technology Landscape

### Trending (gaining momentum)

| Tool/Pattern | Adoption Signal | Key Benefit | Relevance to Grimoire |
|---|---|---|---|
| `vcs.*` and `cicd.*` semconv at Release Candidate | v1.43.0 "promotes VCS attributes, entities, and metrics to release_candidate" ([releases](https://github.com/open-telemetry/semantic-conventions/releases), Summarizer) | Stable-ish names for repo context | OBS-20 (`docs-and-tracing.md:112`) requires semconv names where one exists |
| Opt-in, plaintext, credential-stripped `vcs.*` to an operator collector | Claude Code `OTEL_METRICS_INCLUDE_REPOSITORY`, v2.1.269+ ([monitoring](https://code.claude.com/docs/en/monitoring-usage)) | Operator can group usage by repository | Same activation shape as the discussion's proposal |
| Resource attributes as the operator channel | Claude Code documents `OTEL_RESOURCE_ATTRIBUTES` for team/department/cost centre ([monitoring](https://code.claude.com/docs/en/monitoring-usage)) | No tool code for operator-specific context | Already read by the SDK default Resource |

### Established (proven, widely accepted)

| Tool/Pattern | Status | Notes |
|---|---|---|
| Hash of the git remote as a project id (Next.js, Turborepo, VS Code, Gatsby, Alchemy) | Common for vendor-hosted sinks | Salted in Next.js and Turborepo (carried); unsalted in .NET (reversed in 2020, **[>18mo]**) |
| `url.full` credential redaction | Stable semconv rule | "MUST NOT contain credentials" ([url registry](https://raw.githubusercontent.com/open-telemetry/semantic-conventions/main/model/url/registry.yaml)) |
| `host.id` from machine-id | Semconv recommendation | systemd says the machine id must be hashed with an application-specific key, not used directly ([machine-id(5)](https://www.freedesktop.org/software/systemd/man/latest/machine-id.html), undated) |

### Emerging (early but promising)

| Tool/Pattern | Signal | Worth Watching Because |
|---|---|---|
| `opentelemetry-resource-detectors` `host.name` and container/service-instance detectors | Added in 0.12.0 (2026-08-27, [CHANGELOG](https://raw.githubusercontent.com/open-telemetry/opentelemetry-rust-contrib/main/opentelemetry-resource-detectors/CHANGELOG.md), Summarizer) | The crate's surface is still moving monthly |
| Allowlist-and-mask for unknown hosts (VS Code `getDomainsOfRemotes`) | Source read | Keeps well-known forge domains, masks private ones |
| Root-commit SHA as a repo fingerprint | Alchemy ([privacy](https://alchemy.run/privacy), undated) | Not derived from a URL |

### Declining (losing mindshare)

| Tool/Pattern | Signal | Avoid Because |
|---|---|---|
| Treating an unsalted hash as anonymous | FTC "No, hashing still doesn't make your data anonymous" ([2024-07-24](https://ftc.gov/policy/advocacy-research/tech-at-ftc/2024/07/no-hashing-still-doesnt-make-your-data-anonymous), **[>18mo]**); OTel's own sensitive-data page ([page](https://opentelemetry.io/docs/security/handling-sensitive-data/), undated) | "hashes are reversible in practice if the input space is small and predictable" |
| Collecting argv by default | Semconv "SHOULD NOT be collected by default"; inlang removed its telemetry after `--project /Users/<name>/...` leaked (carried) | The detector crate does it by default |

## Findings

### 1. Semconv registry: names and stability

Source for the whole table: [vcs](https://opentelemetry.io/docs/specs/semconv/registry/attributes/vcs/) and its [raw YAML](https://raw.githubusercontent.com/open-telemetry/semantic-conventions/main/model/vcs/registry.yaml) (two routes), [cicd YAML](https://raw.githubusercontent.com/open-telemetry/semantic-conventions/main/model/cicd/registry.yaml), [user YAML](https://raw.githubusercontent.com/open-telemetry/semantic-conventions/main/model/user/registry.yaml), [enduser YAML](https://raw.githubusercontent.com/open-telemetry/semantic-conventions/main/model/enduser/registry.yaml), [host](https://opentelemetry.io/docs/specs/semconv/registry/attributes/host/), [os](https://opentelemetry.io/docs/specs/semconv/registry/attributes/os/), [process](https://opentelemetry.io/docs/specs/semconv/registry/attributes/process/). All Summarizer reads; registry pages carry no version, the latest release tag is v1.44.0 (4 Aug; the releases page omits the year, 2026 inferred).

| Attribute | Stability | Note on the page (verbatim where quoted) |
|---|---|---|
| `vcs.repository.url.full` | Release Candidate | "In Git Version Control Systems, the canonical URL SHOULD NOT include the `.git` extension." Examples are plain `https://github.com/...` URLs. **No credential or userinfo wording on the page** (two routes). |
| `vcs.repository.name` | Release Candidate | "can clash with forks of the same repository" |
| `vcs.owner.name` | Release Candidate | none |
| `vcs.provider.name` | Release Candidate | well-known: `github`, `gitlab`, `gitea`, `bitbucket` |
| `vcs.ref.head.name`, `vcs.ref.head.type` | Release Candidate | "`head` refers to where you are right now"; type `branch` or `tag` |
| `vcs.ref.head.revision` | Release Candidate | full hash or a non-hash revision (SVN integer); SHOULD be included even if equal to the ref name |
| `vcs.ref.base.*`, `vcs.change.*`, `vcs.revision_delta.direction`, `vcs.line_change.type` | Release Candidate | change/PR oriented; not for a local CLI |
| Entity `vcs.repository` | Release Candidate | identifying `vcs.repository.url.full` (required), descriptive `vcs.repository.name` |
| Entity `vcs.ref` | **Development** | identifying `vcs.ref.head.revision`; its attributes are Release Candidate (stability mismatch inside one namespace, [entities](https://opentelemetry.io/docs/specs/semconv/registry/entities/vcs/)) |
| `vcs.repository.ref.name/.revision/.type`, `vcs.repository.change.*` | Deprecated | replaced by `vcs.ref.head.*` and `vcs.change.*` |
| `cicd.pipeline.name`, `cicd.pipeline.run.id`, `cicd.pipeline.run.url.full`, `cicd.pipeline.task.*`, `cicd.worker.{id,name,url.full,state}`, `cicd.system.component`, `cicd.pipeline.result`, `cicd.pipeline.action.name` | Release Candidate | all `string`; no sensitivity notes. Entities: pipeline (`name` required), pipeline.run (`run.id` required), worker (`id` required). |
| `user.id`, `user.name`, `user.email`, `user.full_name`, `user.roles` | Development | no notes. `user.hash`: "Unique user hash to correlate information for a user in anonymized form", "Useful if `user.id` or `user.name` contain confidential information and cannot be used." The page classifies nothing as PII. |
| `enduser.id` | Development | warning: "This field contains sensitive (PII) information." |
| `enduser.pseudo.id` | Development | warning: "This field contains sensitive (linkable PII) information." |
| `enduser.role` / `enduser.scope` | Deprecated | `user.roles` / removed, no replacement |
| `host.id` | Development | machine-id on non-containerized systems; "SDK resource detector implementations MUST NOT collect `host.id` from privileged sources"; "non-privileged lookups of the machine ID are preferred"; privileged values "should be injected via the `OTEL_RESOURCE_ATTRIBUTES` environment variable" ([host resource](https://opentelemetry.io/docs/specs/semconv/resource/host/)) |
| `host.name` | Development | "On UNIX systems, it may contain what the hostname command returns". Requirement Recommended, **no sensitivity note**. |
| `host.ip`, `host.mac` | Development | Opt-In on the Host entity (the only sensitivity signal on the page) |
| `os.type` | Development | well-known values: `aix`, `darwin`, `dragonflybsd`, `freebsd`, `hpux`, `linux`, `netbsd`, `openbsd`, `solaris`, `windows`, `zos` |
| `process.owner` | Release Candidate | "The username of the user that owns the process." Recommended on the Process entity, no note. Active (not deprecated). |
| `process.command_args`, `process.command_line` | Release Candidate | "SHOULD NOT be collected by default unless there is sanitization that excludes sensitive data." Opt-In on the Process entity ([process resource](https://opentelemetry.io/docs/specs/semconv/resource/process/)). |
| `process.working_directory`, `process.executable.path`, `process.user.name` | Release Candidate | `working_directory` is Opt-In; no notes |
| `url.full` | **Stable** | "MUST NOT contain credentials passed via URL in form of `https://username:password@www.example.com/`"; username and password SHOULD be redacted to `REDACTED:REDACTED`; query values for `sig`, `Signature`, `X-Amz-Signature`, `AWSAccessKeyId` and others SHOULD be replaced by `REDACTED` ([url YAML](https://raw.githubusercontent.com/open-telemetry/semantic-conventions/main/model/url/registry.yaml)) |
| `service.name`, `telemetry.sdk.*` | `telemetry.sdk` Stable | "MUST be provided by the SDK" ([resource index](https://opentelemetry.io/docs/specs/semconv/resource/)) |

Where these attributes are meant to sit:

- The CI/CD spans page (Release Candidate) says "Any resources of the CI/CD and VCS resource conventions that apply SHOULD be used." It does not say whether VCS attributes attach to spans ([cicd-spans](https://opentelemetry.io/docs/specs/semconv/cicd/cicd-spans/), Summarizer).
- The CI/CD resource page carries "using CI/CD pipeline run resource for metrics MUST be opt-in" ([cicd resource](https://opentelemetry.io/docs/specs/semconv/resource/cicd/), Summarizer).
- The CLI-spans page (Development) lists `process.executable.name`, `process.exit.code`, `process.pid` (Required), `process.command_args`, `process.executable.path` (Recommended) and **no** `vcs`, `cicd`, `user` or `host` attribute ([cli-spans](https://opentelemetry.io/docs/specs/semconv/cli/cli-spans/), Summarizer).
- Page inconsistency: the resource index lists Process under "Compute unit" as Development, while the Process entity and the registry say Release Candidate and v1.44.0 notes say the process namespace was promoted ([resource index](https://opentelemetry.io/docs/specs/semconv/resource/), [releases](https://github.com/open-telemetry/semantic-conventions/releases)).

### 2. SDK default Resource and `opentelemetry-resource-detectors`

**SDK default (read in `opentelemetry_sdk-0.33.1`, the version in the local cargo registry and in the earlier research):**

| Fact | Evidence |
|---|---|
| `Resource::builder()` = `SdkProvidedResourceDetector` + `TelemetryResourceDetector` + `EnvResourceDetector` | `src/resource/mod.rs:62-70` |
| The trace provider's default config calls `Resource::builder().build()` | `src/trace/config.rs:36` |
| `service.name` precedence: `OTEL_SERVICE_NAME` (non-empty), else `service.name` inside `OTEL_RESOURCE_ATTRIBUTES`, else `unknown_service:<file name of current_exe()>` (file name only, not the path) | `src/resource/env.rs:75-107`; CHANGELOG line 148-150 |
| `telemetry.sdk.name="opentelemetry"`, `.language="rust"`, `.version=<sdk crate version>` | `src/resource/telemetry.rs` |
| `OTEL_RESOURCE_ATTRIBUTES` is split on `,` then `=`, trimmed, **not percent-decoded** (grep for `percent`/`decode` in `src/resource/` is empty). The spec says "The `,` and `=` characters in keys and values MUST be percent encoded" ([SDK resource spec](https://opentelemetry.io/docs/specs/otel/resource/sdk/), Summarizer); Claude Code's docs show percent-encoded values (`org.name=John%27s%20Organization`) | `src/resource/env.rs:45-58`; changelog line 431 only allows `=` inside a value |
| The module doc says OS and Process detectors "are packaged separately" | `src/resource/mod.rs:13-16` |
| SDK default adds no `host.*`, `os.*`, `process.*` or `vcs.*` | same reads |

**`opentelemetry-resource-detectors` 0.13.0** ([crates.io](https://crates.io/api/v1/crates/opentelemetry-resource-detectors), [README](https://raw.githubusercontent.com/open-telemetry/opentelemetry-rust-contrib/main/opentelemetry-resource-detectors/README.md), [src](https://github.com/open-telemetry/opentelemetry-rust-contrib/tree/main/opentelemetry-resource-detectors/src); source files via Summarizer, line-pattern extractions for `host.rs` and `os.rs`):

| Detector | Attributes | How obtained | Platforms | Semconv sensitivity signal |
|---|---|---|---|---|
| `ProcessResourceDetector` | `process.command_args`, `process.pid`, `process.runtime.{name,version,description}` | `std::env::args_os()`, lossy UTF-8, **no filtering**; runtime name is the literal `"rustc"`, version/description from `option_env!("RUSTC_VERSION*")` set by `build.rs`, omitted if unset | all | `command_args`: "SHOULD NOT be collected by default" |
| `OsResourceDetector` | `os.type` only | `std::env::consts::OS` passed through unchanged; the doc comment says it "maps" to the spec value but the code has no mapping, and the test expects `"macos"` | all | `os.type` enum lists `darwin`, not `macos` ([std OS consts](https://doc.rust-lang.org/std/env/consts/constant.OS.html) lists `macos`) |
| `HostResourceDetector` | `host.arch`, `host.name`, `host.id` | `host.arch` = compile-time `ARCH` (the build target, not the running CPU); `host.name` = stdout of `Command::new("hostname")`, dropped if empty (added in 0.12.0); `host.id` = `/etc/machine-id` then `/var/lib/dbus/machine-id` on Linux, `ioreg -rd1 -c IOPlatformExpertDevice` on macOS, `None` elsewhere | `host.name`/`arch`: all; `host.id`: README says linux, source also has macOS | systemd: machine id "must not be exposed in untrusted environments", "must not be used directly", hash it "using a fixed, application-specific key"; semconv: detectors MUST NOT use privileged sources (machine-id files are world-readable, so allowed) |
| `K8sResourceDetector` | `k8s.pod.name`, `k8s.namespace.name` | env/files | all | none read |
| `ContainerResourceDetector` | `container.id` | `/proc/self/cgroup`, fallback `/proc/self/mountinfo` | linux | none read |
| `ServiceInstanceIdResourceDetector` | `service.instance.id` | random UUIDv7 generated per call (Inference: a fresh id for every CLI process) | all | none read |

Crate facts:

- **Maturity.** README says "beta"; first release 2024-03-19 **[>18mo]**; 12 releases; latest 0.13.0 on 2026-09-21; 3,148,563 total and 705,543 recent downloads; owner listed as a single GitHub user; rust-version 1.75.0; Apache-2.0 ([crates.io](https://crates.io/api/v1/crates/opentelemetry-resource-detectors)). It bumps its `opentelemetry` pin with every minor (0.29 at 0.8.0, 0.33 at 0.13.0), so it must move in lockstep with `opentelemetry-otlp`.
- **Dependency weight.** Normal dependencies: `opentelemetry` (default-features off), `opentelemetry_sdk`, `opentelemetry-semantic-conventions` with `semconv_experimental` ([deps](https://crates.io/api/v1/crates/opentelemetry-resource-detectors/0.13.0/dependencies)). The semconv crate is 124,588 bytes packaged, has no normal dependencies ([crate](https://crates.io/api/v1/crates/opentelemetry-semantic-conventions/0.33.1), [deps](https://crates.io/api/v1/crates/opentelemetry-semantic-conventions/0.33.1/dependencies)), and is not a dependency of `opentelemetry-otlp`/`-sdk`/`opentelemetry` 0.33.1 (Cargo.toml grep, empty) nor present in the local cargo registry. So the net addition over the discussion's +24 crates is two crates (the detectors crate and the constants crate), not a `hostname` crate: grim's `Cargo.toml`/`Cargo.lock` hold no `hostname`/`gethostname`.
- **Subprocess cost.** `host.name` spawns `hostname`, and macOS `host.id` spawns `ioreg`. For comparison, 100 sequential `git config --get remote.origin.url` on this WSL2 machine (git 2.54.0, warm cache) took 0.074 s total (about 0.7 ms each); not representative of Windows or a cold cache.
- **Doing without the crate.** Every attribute above except `process.runtime.*` and `host.id` can be set by code from `std::env::consts`, `std::process::id()` or an `OTEL_RESOURCE_ATTRIBUTES` entry; `service.name` and `telemetry.sdk.*` need nothing (Inference from the table).

### 3. How comparable tools detect repository context

| Tool | Mechanism | Normalization / credentials | Sent as | Default |
|---|---|---|---|---|
| Claude Code ([monitoring](https://code.claude.com/docs/en/monitoring-usage); read the first 100,000 of 156,786 characters) | reads the `origin` remote once per session | HTTPS and SSH forms of one repo give identical values; "Values are lowercased, and credentials, query strings, and fragments from the remote URL never appear in them"; omitted with no `origin`, a non-URL remote, or when "the only enclosing repository is your home directory" | `vcs.repository.url.full` (browser URL, no `.git`), `vcs.owner.name`, `vcs.repository.name`, `vcs.provider.name`; **not hashed**; "Anthropic's telemetry drops every `vcs.*` key" | off; `OTEL_METRICS_INCLUDE_REPOSITORY=true`, v2.1.269+. A `vcs.*` key in `OTEL_RESOURCE_ATTRIBUTES` replaces the derived value; declaring `vcs.repository.url.full` stops the remote read |
| Next.js ([project-id.ts](https://raw.githubusercontent.com/vercel/next.js/canary/packages/next/src/telemetry/project-id.ts), Summarizer) | `git config --local --get remote.origin.url`, 1000 ms timeout | the excerpt returns `String(...).trim()` with no credential stripping; the comment says "hashed one-way" with "random salt data"; fallback chain `REPOSITORY_URL` env, then `process.cwd()` | hashed project id (carried) | opt-out |
| VS Code ([docs, page dated 2026-10-21](https://code.visualstudio.com/docs/configure/telemetry); [workspaceTagsService](https://raw.githubusercontent.com/microsoft/vscode/main/src/vs/workbench/contrib/tags/common/workspaceTags.ts); [configRemotes.ts](https://raw.githubusercontent.com/microsoft/vscode/main/src/vs/platform/extensionManagement/common/configRemotes.ts), Summarizer) | regex over `.git/config` text (`url = ...`) | URL form: `user:pass@` and port dropped; scp form uses `^([^@:]+@)?([^:]+):(.+)$`, so a `user:pass@host:path` value is not covered (per the extract); no lowercasing; `.git` stripped only when the caller asks | "Specific folders are identified by computing a hash of each of the folder's Git remotes" (`sha1Hex` callback by parameter name; the algorithm is not shown); a second function reduces hosts to second-level domains and masks any not on an allowlist (`internal.corp` becomes `aaaaaaaa.aaaa`) | level-gated (`telemetry.telemetryLevel`) |
| `dd-trace-js` ([url.js](https://raw.githubusercontent.com/DataDog/dd-trace-js/master/packages/dd-trace/src/plugins/util/url.js), Summarizer) | CI env first, then local `.git` (Datadog docs via search snippet) | `filterSensitiveInfoFromRepository`: `ssh://user@` username removed; other URLs rebuilt from protocol+host+pathname (userinfo dropped); a scp-style `git@host:path` is returned unchanged | `git.repository_url` | n/a |
| Alchemy CLI ([privacy](https://alchemy.run/privacy), undated, Summarizer) | `git config remote.origin.url`, branch, first commit | `alchemy.git.origin_hash` "SHA-256 of `git config remote.origin.url`. The raw URL is never sent."; `branch_hash`; `root_commit` "SHA of the first commit in your repo. Acts as a stable repo fingerprint."; salt not stated | OTLP exporter; `DO_NOT_TRACK=1` "disables the OTLP exporter entirely"; random UUID user id, "Not your shell user, email, or hostname" | on; v1 policy differs, and a 0.93.9 source type still declares `gitOriginUrl` (unresolved, [source](https://cdn.jsdelivr.net/npm/alchemy@0.93.9/src/util/telemetry.ts)) |
| Pants ([anonymous telemetry, 2.34 dev docs, undated](https://www.pantsbuild.org/dev/docs/using-pants/anonymous-telemetry)) | not derived: the repo owner writes a random `repo_id` (for example from `uuidgen`) in `pants.toml` | sent as "the sha256 hash of the repo id"; "For repos that are publicly visible ... the `repo_id` will be visible in your `pants.toml`. So repo-level data is not anonymous." Machine id = sha256(repo id + MAC) | run id, duration, outcome, platform | opt-in |
| Moderne CLI ([telemetry](https://docs.moderne.io/user-documentation/moderne-cli/how-to-guides/cli-telemetry), Summarizer) | per-command trace files in `.moderne/` | `repository.branch`, `cloneUri`, `repository.origin` recorded; CSV `developer` column holds "git email of the developer who ran the command"; a log file path is stripped from central copies, a source file path is not | local JSON/CSV; tenant gateway upload when signed in; not OTLP | the page states no opt-out |
| Bazel/BuildBuddy ([guide-metadata](https://www.buildbuddy.io/docs/guide-metadata), search snippet) | `--build_metadata=REPO_URL=...` or a `workspace_status_command` script; can read `GITHUB_REPOSITORY`/`BUILDKITE_REPO` | plaintext repo URL, branch and commit SHA to the backend the operator names; Bazel's default status keys include `BUILD_HOST` and `BUILD_USER` ([Aspect docs](https://docs.aspect.build/rulesets/aspect_bazel_lib/docs/stamping), search snippet, whether BuildBuddy forwards them not confirmed) | BES upload | operator-configured |
| GitHub CLI ([telemetry docs](https://docs.github.com/en/github-cli/github-cli/github-cli-telemetry), 2026, Summarizer) | n/a for commands | `command_invocation` dimensions: `agent, architecture, ci, command, device_id, flags, github_actions, invocation_id, is_tty, os, timestamp, version`; `skill_install` adds `skill_owner`, `skill_repo`, `repo_visibility` (the installed skill's source; how visibility is determined is not stated); no URL/branch/commit/user/host/cwd in the example payload (not an explicit exclusion) | pseudonymous | on; `GH_TELEMETRY=0`, `DO_NOT_TRACK=true` |
| Codex ([config-advanced](https://learn.chatgpt.com/docs/config-file/config-advanced), reached by a followed 308 redirect from developers.openai.com, Summarizer) | n/a | no repo name, remote or cwd path; one boolean "tagged by whether the working directory is in a Git repo" | `auth_mode`, `model`, `app.version`, `originator`, `session_source` | export off |
| .NET SDK ([Learn, updated 2026-10-02](https://learn.microsoft.com/en-us/dotnet/core/tools/telemetry)) | n/a | "doesn't extract project-level data, such as name, repository, or author"; collects a hashed project identifier for `dotnet run` (10.0.100+), hashed cwd and hashed MAC (SHA256) (2.0+); 11.0.200 adds a normalized local employee alias and corporate domain for a positive Microsoft-employee result outside CI | Azure Monitor | on, `DOTNET_CLI_TELEMETRY_OPTOUT` |
| Gemini CLI ([telemetry](https://google-gemini.github.io/gemini-cli/docs/cli/telemetry.html), Summarizer) | n/a | only `sessionId` is named as a common attribute; no host/os/git attribute documented | | off |

**How the remote is read matters (local test, git 2.54.0, throwaway repo):** with `url."https://tok123@github.com/".insteadOf "https://github.com/"` set globally, `git config --get remote.origin.url` printed `https://github.com/acme/repo.git`, while `git remote get-url origin` and `git ls-remote --get-url origin` both printed `https://tok123@github.com/acme/repo.git`. grim's provenance code uses the `config --get` form (`src/oci/git_provenance.rs:199`), then `normalize_remote_url` drops userinfo and ports, upgrades `http://` to `https://`, rewrites scp and `ssh://` forms, returns `None` for `file://`, bare paths and Windows drive paths, and funnels every shape through one helper (`:363-421`).

Real credential-bearing remote URLs:

- GitLab: `CI_REPOSITORY_URL` is "The full path to Git clone (HTTP) the repository with a CI/CD job token, in the format `https://gitlab-ci-token:$CI_JOB_TOKEN@gitlab.example.com/my-group/my-project.git`" ([GitLab docs, raw](https://gitlab.com/gitlab-org/gitlab/-/raw/master/doc/ci/variables/predefined_variables.md), Summarizer).
- Bitrise Build Cache analytics stored repository URLs; on 2026-07-15 it announced that "Some private repository URLs can contain embedded access tokens" and that embedded credentials are now stripped before storage ([post](https://discuss.bitrise.io/t/secret-redaction-in-repo-urls-build-cache-analytics/26075), Summarizer). The post does not describe an incident.

### 4. Privacy evidence by data class

| Data class | Collected anywhere? | Treatment found | Hazard evidence |
|---|---|---|---|
| Repository URL | Claude Code (opt-in, plaintext), Moderne (plaintext), BuildBuddy (plaintext), VS Code/Next.js/Alchemy/Gatsby (hashed) | credential stripping: Claude Code, dd-trace, VS Code (URL form), grim's `normalize_remote_url`; none visible in the Next.js excerpt | embedded tokens: GitLab doc, Bitrise; normalizer gaps in dd-trace (`git@` kept) and VS Code (scp form) |
| Branch name | Alchemy (SHA-256), Moderne (plaintext, commit/push targets) | hash or plaintext | none found as an incident |
| Commit SHA / revision | semconv `vcs.ref.head.revision`; Alchemy `root_commit`; Moderne build `commit SHA` | grim publishes HEAD SHA and commit date by default because they describe content (`git_provenance.rs:23-31`) | root commit of a public repo is itself public (Inference) |
| Author name / email | Moderne `developer` (git email), Claude Code `user.email` "always when available", Microsoft alias in .NET 11.0.200 | grim reads `%an` but clears it outside `--git` (`git_provenance.rs:206-209, 228-231, 265-271`); never `%ae` | semconv has `user.email`/`user.full_name` at Development with no PII note; `enduser.id` carries the PII warning |
| Hostname | semconv `host.name`; the detector crate (since 0.12.0); inlang's Sentry reports (carried) | Alchemy and releases.sh say they do not collect it; Gatsby/Astro lists (carried) | detector spawns `hostname` by default; semconv gives no sensitivity note |
| Username | `process.owner` (Recommended), `process.user.name`; .NET alias (positive classification only) | Claude Code generates a random `user.id` stored in `~/.claude.json`, "not derived from your Claude account" | none found for the OTel attributes themselves |
| Working directory | `process.working_directory` (Opt-In); .NET hashes it; Next.js uses raw cwd as the last fallback before hashing | hashing | .NET unsalted cwd hash reproduced with `sha256sum` (2020, **[>18mo]**, carried); inlang path leak (carried) |
| argv | `process.command_args` (Opt-In, "SHOULD NOT ... by default"); the detector crate emits it raw | Sanity allowlist, Go allowlist (carried) | inlang `--project /Users/<name>/...` (carried) |
| Machine id | `host.id`; the detector reads the raw file | systemd: hash with an application-specific key; .NET and Pants hash MAC | n/a |

Employer-held data: Bazel/BuildBuddy and Moderne are designed for an operator who sees per-developer and per-repo data. No developer-tool-specific source on works-council or GDPR treatment of such telemetry was found (see Leads). A German-law overview notes that data which allows conclusions about behavior or performance can trigger co-determination under § 87(1) no. 6 BetrVG even when "neutral" ([Luther](https://www.luther-lawfirm.com/en/newsroom/blog/detail/dauerbrenner-software-vs-mitbestimmung-87-abs-1-nr-6-betrvg), secondary; page not opened, from a search summary).

### 5. Hashing evidence

| Scheme | Who | What the sources say |
|---|---|---|
| Unsalted hash of a guessable input | .NET (cwd, MAC), Alchemy (origin URL, branch), VS Code (remotes; salt not shown), OTel collector `attributes` `hash` action ("Hashes (SHA1) an existing attribute value", salt not stated, [README](https://raw.githubusercontent.com/open-telemetry/opentelemetry-collector-contrib/main/processor/attributesprocessor/README.md)) | OTel: "hashes are reversible in practice if the input space is small and predictable". FTC: "hashes aren't 'anonymous' and can still be used to identify users"; "it still creates a unique signature that can track a person or device over time" (does not mention salting). dgl reproduced the .NET hash (carried, **[>18mo]**). |
| Per-machine random salt, never sent | Next.js, Turborepo (carried) | Not joinable across machines (carried) |
| Owner-chosen random id, then hashed | Pants | Public repos expose the id; "repo-level data is not anonymous" |
| Keyed hash | Collector `redaction` processor `hash_function`: md5, sha1, sha3, hmac-sha256, hmac-sha512; recommends the HMAC variants "especially when dealing with low-entropy data" ([README](https://github.com/open-telemetry/opentelemetry-collector-contrib/blob/main/processor/redactionprocessor/README.md), traces Beta, logs and metrics Alpha); systemd `sd_id128_get_machine_app_specific` | Key lives with the operator or the application |
| Collector-side options | `redaction` (allowlist, fails closed on empty `allowed_keys`), `attributes` (hash, delete), `transform` (SHA256), `filter` ([handling sensitive data](https://opentelemetry.io/docs/security/handling-sensitive-data/)) | The page puts responsibility on "the implementer" and says "the best way to prevent the collection of sensitive data is not to collect data that might be sensitive" |
| Authorities not read | EDPB Guidelines 01/2025 on pseudonymisation (adopted after a consultation closing 2025-03-14); search returned only the landing page and a vendor blog | Content on hashing and salts unverified |

No CLI was found that hashes a repository identifier with a key supplied by the operator.

## Evidence by option

### A. No VCS context

| For | Against |
|---|---|
| Codex attaches only a boolean "working directory is in a Git repo", and the .NET page says it does not extract repository data. | An operator who asks "which repositories use grim" cannot answer it from the record; Claude Code ships an opt-in toggle for exactly this, and BuildBuddy "groups invocations by repository". |
| grim's own publish path treats the origin URL and the author as disclosure-sensitive and withholds them by default (`git_provenance.rs:23-31`, `:228-231`). | A fleet-wide environment value is per machine or shell, not per repository (Inference), so `OTEL_RESOURCE_ATTRIBUTES` alone cannot supply a per-repo value. |
| No git subprocess, no credential-stripping surface, no gap class like dd-trace's `git@` passthrough or VS Code's scp pattern. | |
| No semconv sensitivity guidance to interpret: the registry is silent on `vcs.*`. | |

### B. Credential-stripped origin URL (plaintext)

| For | Against |
|---|---|
| Claude Code ships this shape: opt-in, normalized, credentials/query/fragment stripped, omitted for a home-directory repo, to an operator-owned sink. | Credentials in remote URLs are documented (GitLab `CI_REPOSITORY_URL`, Bitrise); shipping normalizers have gaps (dd-trace keeps `git@host:path`; VS Code's scp pattern).  |
| Semconv names exist at Release Candidate (`vcs.repository.url.full`, `vcs.owner.name`, `vcs.repository.name`, `vcs.provider.name`). | Semconv gives no credential rule for `vcs.repository.url.full`; the only rule is on `url.full` (Stable). OBS-22 (`docs-and-tracing.md:114`) requires a redactor for any recorded URL, and none exists yet. |
| grim already has the subprocess and a tested stripper (`git_provenance.rs:182-217, 363-421`); reading via `config --get` ignores `insteadOf` token rewrites (local test above). | A private repository's host and path can identify a customer or project; the discussion's own #83 scope excluded "registry hosts, workspace paths" (carried, codebase recon finding 6). |
| Moderne and BuildBuddy send plaintext repo identity to operator-chosen backends; Pants shows hashing public repos buys little. | `vcs.repository.name` "can clash with forks"; "origin" is a convention (forks often use `upstream`), and git honours `GIT_*` environment overrides that a repo-set `.envrc` could supply (Inference, untested). |
| | A high-cardinality URL as a metric dimension is the risk spanmetrics defaults leave open (default limit 0, carried); CI/CD resource for metrics MUST be opt-in (semconv). |

### C. Hashed repository id

| For | Against |
|---|---|
| Common ecosystem pattern for vendor-hosted sinks: Next.js, Turborepo, VS Code, Gatsby, Alchemy, .NET project id. | Unsalted hashes of guessable inputs are treated as reversible by OTel's guidance, the FTC and the .NET incident; public repo URLs can be enumerated (Astro source comment, carried; Pants doc). |
| Hides the name from anyone with read access to the backend but not the key (Inference). | Per-machine salt (Next.js, Turborepo) cannot be joined across machines; a shared salt or key must live somewhere (no CLI found with an operator-supplied key). |
| Root-commit SHA (Alchemy) needs no URL and is stable across clones and forks. | The collector can already hash or drop attributes (`attributes`, `transform`, `redaction` with HMAC), so a CLI-side hash and a collector-side hash overlap when the sink is operator-owned (Inference). |
| Pants' owner-chosen random id avoids deriving anything from the repo. | A hash does not remove the URL read, so the credential-stripping and git-subprocess surface of option B remains for the hashed variant (Inference). |

### D. Operator-injected attributes only (`OTEL_RESOURCE_ATTRIBUTES`)

| For | Against |
|---|---|
| Already read by the SDK default Resource (`env.rs:19-30`); no grim code to detect or normalize anything. | Env is repo-settable (the project's own T3/CWE-426 reasoning, `.claude/rules/arch-threat-model.md`); Claude Code 2.1.282 ignores project-level OTel variables for this reason (carried). |
| Semconv itself tells operators to inject privileged `host.id` values this way; Claude Code documents it for team/department/cost centre and lets a `vcs.*` entry override a derived value. | Rust SDK 0.33.1 does not percent-decode values although the spec requires encoding `,` and `=`; a value copied from Claude Code's documented form would arrive encoded. |
| Operator decides exactly which keys exist; no maintainer PII decision. | Every key becomes a resource attribute on the record; Claude Code notes each custom key becomes a metric label and that high cardinality raises cost. |
| | Per machine or shell, not per repo or per run (Inference); no `service.name` fallback concern, since the default is `unknown_service:grim`. |

The options are not exclusive in the one shipping precedent: Claude Code derives (B) and lets an operator-injected value (D) replace it.

## Design Patterns Worth Considering

- **Derive once, omit when ambiguous.** Claude Code reads `origin` once per session and omits the attributes for no origin, a non-URL remote, or a home-directory repository. Used by: Claude Code ([monitoring](https://code.claude.com/docs/en/monitoring-usage)).
- **Funnel normalization through one function.** grim's `https_from_authority_and_path` strips userinfo and port for every remote shape (`git_provenance.rs:398-421`); dd-trace has two code paths and one gap.
- **Mask unknown hosts, keep known forges.** VS Code reduces hosts to second-level domains and masks any not on an allowlist (`configRemotes.ts`).
- **Id the repo by something the owner chose.** Pants `repo_id` (random UUID in the repo's own config).
- **Read the remote without `insteadOf`.** `git config --get remote.origin.url` (local test above).
- **Operator override beats derivation.** Claude Code: a `vcs.*` key in `OTEL_RESOURCE_ATTRIBUTES` replaces the derived value, and declaring the URL stops the remote read.

## Key Findings

1. The Rust SDK default Resource holds `service.name`, `telemetry.sdk.*` and `OTEL_RESOURCE_ATTRIBUTES` only (`opentelemetry_sdk-0.33.1/src/resource/mod.rs:62-70`, `env.rs:75-107`, `trace/config.rs:36`).
2. `opentelemetry-resource-detectors` 0.13.0 collects raw argv, `hostname` output and the raw machine-id by default, against semconv's "SHOULD NOT be collected by default" for argv and systemd's "must not be used directly" for the machine id ([README](https://raw.githubusercontent.com/open-telemetry/opentelemetry-rust-contrib/main/opentelemetry-resource-detectors/README.md), [machine-id(5)](https://www.freedesktop.org/software/systemd/man/latest/machine-id.html)).
3. Its `os.type` is `std::env::consts::OS` unmapped ("macos") while the semconv value is `darwin` (detector `os.rs` extract; [os registry](https://opentelemetry.io/docs/specs/semconv/registry/attributes/os/)).
4. `vcs.*` and `cicd.*` reached Release Candidate in v1.43.0; `process.*` in v1.44.0; `user.*`, `host.*` and `os.*` remain Development ([releases](https://github.com/open-telemetry/semantic-conventions/releases), registry pages).
5. The semconv registry has no credential rule for `vcs.repository.url.full`; the Stable `url.full` does ([url YAML](https://raw.githubusercontent.com/open-telemetry/semantic-conventions/main/model/url/registry.yaml)).
6. Claude Code is the primary precedent for auto-detected, credential-stripped, plaintext `vcs.*` to an operator collector, opt-in ([monitoring](https://code.claude.com/docs/en/monitoring-usage)).
7. grim already detects git and normalizes remotes (`src/oci/git_provenance.rs:182-217, 363-421`) and withholds origin and author by default when publishing (`:23-31, :228-231`).
8. Credentials in remote URLs are documented (GitLab `CI_REPOSITORY_URL`, Bitrise); shipping normalizers have gaps (dd-trace `git@` passthrough, VS Code scp pattern).
9. OTel's guidance, the FTC and the .NET incident treat unsalted hashes of guessable inputs as reversible or identifying; no CLI was found hashing a repo id with an operator-supplied key.
10. Update to `research_otel_community_threads.md` (line 121, gh "Docs list flag names only"): the current gh telemetry doc also shows a `skill_install` event with `skill_owner`, `skill_repo` and `repo_visibility` ([docs](https://docs.github.com/en/github-cli/github-cli/github-cli-telemetry)). It is the source of an installed skill, not the user's repository.

## Recommendation

None by design. The lane brief asks for evidence for and against four options; the "Evidence by option" tables are the input.

## Negatives (searched, not found)

- A semconv rule, issue or note about credentials or userinfo in `vcs.repository.url.full`; a web search for one found none and the registry text has none.
- `vcs`, `cicd`, `user` or `host` attributes on the CLI-spans page ([cli-spans](https://opentelemetry.io/docs/specs/semconv/cli/cli-spans/)); no `cli.*` namespace.
- A CLI that hashes a repository id with an operator-supplied key.
- An incident of telemetry leaking git author email, branch names or hostnames: a search found none (inlang's hostname in Sentry reports is carried). The nearest are Bitrise's warning about stored URLs, which describes no incident, and the .NET and inlang path leaks.
- Datadog docs on userinfo handling for `git.repository_url`: none in search results; the code does strip it.
- Sensitivity wording on `host.name`, `process.owner` and `user.email`: none in the registry.
- Issues in `opentelemetry-rust-contrib` about `host.name`, `hostname` or `process.command_args`: none seen in the first 100,000 of 332,158 characters of the issue list.
- Codex and Gemini CLI docs list no repository name, remote, host or username attribute (Gemini's page names only `sessionId`; it may be incomplete).
- `docs.homebrew.sh` redirects to an unrelated advertising domain; not followed. The real page is `docs.brew.sh/Analytics` (carried).
- The works-council search returned general § 87 BetrVG sources and nothing on developer-tool telemetry.

## Open leads

- **security-threat-model:** the origin URL of an untrusted clone is T3 input (arbitrary string, length, control characters), and git subprocess behavior responds to `GIT_*` environment a repo can set; neither was tested here.
- **privacy-legal:** employer-held per-developer telemetry (works council, GDPR pseudonymisation); EDPB Guidelines 01/2025 content on hashing and salts was not read.
- **crate-feasibility:** build `opentelemetry-resource-detectors` 0.13.0 in a scratch crate and read the real `host.rs`/`os.rs`/`process.rs` (this lane used the WebFetch summarizer), including the README-vs-source disagreement on `host.id` platforms and whether `hostname` is absent in minimal containers.
- **prior-art:** read the unread 56,786 characters of the Claude Code monitoring page, Alchemy's `Telemetry/Attributes.ts`, and VS Code's workspace-tag property names and hash algorithm.
- **perf-measurement:** cost of a git subprocess (and `hostname`) on Windows and macOS and with a cold cache; the one local number here is WSL2 warm.
- **attribute-set-design:** whether `service.instance.id`, a per-process random UUID, is wanted on a one-record-per-run CLI; and which attributes belong on the resource versus the span given the semconv CI/CD and VCS resource pages.

## Method notes

- Web fetches went through the WebFetch summarizing model, so quotes are extractions; load-bearing claims were read on two routes where possible (vcs, process, host, url). Single-route claims: `host.rs`, `os.rs`, `process.rs` of the detectors crate, the VS Code, dd-trace and Next.js sources, the Moderne and Alchemy pages.
- A direct `curl` of raw files was blocked by the sandbox, so raw GitHub YAML and source were fetched through WebFetch.
- Local checks: the SDK source in `~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/opentelemetry_sdk-0.33.1/`, grim's `src/oci/git_provenance.rs`, and a throwaway git repository under the session scratchpad for the `insteadOf` and timing tests.

## Sources

| Source | Type | Date | Relevance |
|--------|------|------|-----------|
| https://opentelemetry.io/docs/specs/semconv/registry/attributes/vcs/ | Spec | undated (v1.44.0 era) | `vcs.*` names and stability |
| https://raw.githubusercontent.com/open-telemetry/semantic-conventions/main/model/vcs/registry.yaml | Spec source | current | `vcs.*` notes, second route |
| https://opentelemetry.io/docs/specs/semconv/registry/entities/vcs/ | Spec | undated | `vcs.repository` RC, `vcs.ref` Development |
| https://raw.githubusercontent.com/open-telemetry/semantic-conventions/main/model/cicd/registry.yaml | Spec source | current | `cicd.*` |
| https://opentelemetry.io/docs/specs/semconv/resource/cicd/ | Spec | undated | CI/CD and VCS resource entities, metrics opt-in |
| https://opentelemetry.io/docs/specs/semconv/cicd/cicd-spans/ | Spec | undated | CI/CD spans RC, resource reference |
| https://raw.githubusercontent.com/open-telemetry/semantic-conventions/main/model/user/registry.yaml | Spec source | current | `user.*` Development, `user.hash` |
| https://raw.githubusercontent.com/open-telemetry/semantic-conventions/main/model/enduser/registry.yaml | Spec source | current | `enduser.*` PII warnings |
| https://opentelemetry.io/docs/specs/semconv/registry/attributes/host/ , https://opentelemetry.io/docs/specs/semconv/resource/host/ | Spec | undated | `host.name`, `host.id` privileged-source rule |
| https://opentelemetry.io/docs/specs/semconv/registry/attributes/os/ | Spec | undated | `os.type` well-known values |
| https://opentelemetry.io/docs/specs/semconv/registry/attributes/process/ , https://opentelemetry.io/docs/specs/semconv/resource/process/ | Spec | undated | `process.*` RC, argv sanitization |
| https://opentelemetry.io/docs/specs/semconv/cli/cli-spans/ | Spec | undated | CLI span attributes |
| https://raw.githubusercontent.com/open-telemetry/semantic-conventions/main/model/url/registry.yaml | Spec source | current | `url.full` credential rule |
| https://opentelemetry.io/docs/specs/semconv/resource/ | Spec | undated | stability by group, SDK MUSTs |
| https://github.com/open-telemetry/semantic-conventions/releases | Releases | v1.41.0 to v1.44.0, 2026 (year inferred) | promotion of vcs, cicd, process |
| https://opentelemetry.io/docs/specs/otel/resource/sdk/ | Spec | undated | `OTEL_RESOURCE_ATTRIBUTES` encoding, detector rules |
| https://opentelemetry.io/docs/languages/sdk-configuration/general/ | Docs | undated | `OTEL_SERVICE_NAME`, `OTEL_TRACES_EXPORTER` defaults |
| https://opentelemetry.io/docs/security/handling-sensitive-data/ | Docs | undated | hashing caveat, collector techniques |
| https://github.com/open-telemetry/opentelemetry-collector-contrib/blob/main/processor/redactionprocessor/README.md | Docs | current | HMAC hash, allowlist |
| https://raw.githubusercontent.com/open-telemetry/opentelemetry-collector-contrib/main/processor/attributesprocessor/README.md | Docs | current | `hash` action SHA1 |
| https://crates.io/api/v1/crates/opentelemetry-resource-detectors (+ `/0.13.0/dependencies`) | Registry | updated 2026-09-21 | version history, deps, downloads |
| https://crates.io/api/v1/crates/opentelemetry-semantic-conventions (+ `/0.33.1`, `/0.33.1/dependencies`) | Registry | 2026-10-09 | constants crate size and deps |
| https://github.com/open-telemetry/opentelemetry-rust-contrib/tree/main/opentelemetry-resource-detectors | Repo | current | detector list, beta |
| https://raw.githubusercontent.com/open-telemetry/opentelemetry-rust-contrib/main/opentelemetry-resource-detectors/{README.md,CHANGELOG.md,Cargo.toml,src/lib.rs,src/process.rs,src/host.rs,src/os.rs} | Source | current | what each detector collects |
| `~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/opentelemetry_sdk-0.33.1/src/resource/{mod,env,telemetry}.rs`, `src/trace/config.rs`, `CHANGELOG.md` | Local source | 0.33.1 | SDK default Resource |
| https://doc.rust-lang.org/std/env/consts/constant.OS.html | Docs | current | `macos` value |
| https://www.freedesktop.org/software/systemd/man/latest/machine-id.html | Man page | undated | machine id confidentiality, keyed hash |
| https://code.claude.com/docs/en/monitoring-usage | Docs | current (first 100,000 of 156,786 characters) | `vcs.*` opt-in, normalization, `OTEL_RESOURCE_ATTRIBUTES` |
| https://raw.githubusercontent.com/vercel/next.js/canary/packages/next/src/telemetry/project-id.ts | Source | current | `git config --local --get remote.origin.url`, fallback chain |
| https://code.visualstudio.com/docs/configure/telemetry | Docs | 2026-10-21 | hash of each Git remote |
| https://raw.githubusercontent.com/microsoft/vscode/main/src/vs/workbench/contrib/tags/common/workspaceTags.ts , https://raw.githubusercontent.com/microsoft/vscode/main/src/vs/platform/extensionManagement/common/configRemotes.ts | Source | current | remote parsing, credential handling, host masking |
| https://raw.githubusercontent.com/DataDog/dd-trace-js/master/packages/dd-trace/src/plugins/util/url.js | Source | current | `filterSensitiveInfoFromRepository` |
| https://alchemy.run/privacy | Docs | undated | hashed origin and branch, root commit |
| https://www.pantsbuild.org/dev/docs/using-pants/anonymous-telemetry | Docs | 2.34 dev, undated | owner-chosen repo id, public-repo caveat |
| https://docs.moderne.io/user-documentation/moderne-cli/how-to-guides/cli-telemetry | Docs | undated | plaintext branch, clone URI, git email |
| https://www.buildbuddy.io/docs/guide-metadata | Docs | undated (search snippet) | operator-supplied repo metadata |
| https://docs.github.com/en/github-cli/github-cli/github-cli-telemetry | Docs | 2026 | gh payload fields incl. `skill_install` |
| https://learn.chatgpt.com/docs/config-file/config-advanced | Docs | undated (reached by a followed redirect) | Codex default metadata |
| https://google-gemini.github.io/gemini-cli/docs/cli/telemetry.html | Docs | undated | Gemini common attributes |
| https://learn.microsoft.com/en-us/dotnet/core/tools/telemetry | Docs | updated 2026-10-02 | hashed cwd, project id, "no repository" statement |
| https://gitlab.com/gitlab-org/gitlab/-/raw/master/doc/ci/variables/predefined_variables.md | Docs | current | `CI_REPOSITORY_URL` embeds the job token |
| https://discuss.bitrise.io/t/secret-redaction-in-repo-urls-build-cache-analytics/26075 | Forum | 2026-07-15 | credential stripping in stored repo URLs |
| https://ftc.gov/policy/advocacy-research/tech-at-ftc/2024/07/no-hashing-still-doesnt-make-your-data-anonymous | Regulator blog | 2024-07-24 **[>18mo]** | hashing is not anonymization |
| https://www.luther-lawfirm.com/en/newsroom/blog/detail/dauerbrenner-software-vs-mitbestimmung-87-abs-1-nr-6-betrvg | Law firm blog | undated (search snippet only) | § 87(1) no. 6 BetrVG breadth |
| `src/oci/git_provenance.rs` | Local code | read 2026-10-22 | existing git detection and normalizer |
| `.claude/rules/rust-quality/docs-and-tracing.md:112,114` | Local rule | read 2026-10-22 | OBS-20, OBS-22 |
| `.claude/rules/arch-threat-model.md` | Local rule | read 2026-10-22 | T3 repo-supplied input |
| `.agents/research/research_otel_{codebase_recon,cli_prior_art,community_threads}.md` | Prior research | 2026-10-22 | carried facts |
