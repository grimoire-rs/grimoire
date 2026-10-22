# Research: CLI telemetry — community reception

<!--
Technology Landscape Research
Filename: .agents/research/research_otel_community_threads.md
Owner: Researcher (hex research phase)
Handoff to: Architect (/hex-architect), /hex-plan
Related Skills: hex-architect, hex-plan

Purpose: Persist tech landscape findings to inform ADRs, plans, design decisions.
Artifacts decay — check dates before trusting findings.
-->

## Metadata

**Date:** 2026-10-22
**Domain:** observability | cli | security
**Triggered by:** `.agents/discussions/otel-usage-telemetry.md` ([grimoire-rs/grimoire#206](https://github.com/grimoire-rs/grimoire/issues/206)) — opt-in OTel export of per-invocation usage records, env-var configured, operator-owned collector, no vendor default. Lane: community threads and blogs.
**Expires:** 2027-04-22 (several rows are weeks old: gh CLI, Claude Code, Sanity, inlang)

Conventions: **[>18mo]** marks a source older than about 2025-04-22. "Secondary" marks a claim read from a search snippet, mirror or summary rather than the primary page. Quotes are from the fetched page unless noted.

## Direct Answer

No recommendation (lane brief). Five patterns recur in the evidence, each with support and counter-evidence in "Evidence for and against" below:

1. **Default-on drew the objections; the fixes that held were opt-in or local-first.** Go reversed to opt-in after the backlash; .NET, Homebrew and (2026) the GitHub CLI stayed opt-out and the objections recurred years later.
2. **Operator-owned export is already shipping in AI-agent CLIs** (Claude Code, Codex, Copilot CLI, Gemini CLI) with off-by-default switches. Evidence that platform teams want it is mostly issue-tracker requests from those tools, not blog essays.
3. **Ambient `OTEL_*` variables are a documented source of unwanted export, delay and cross-tool leakage** (Claude Code, buildx, Terraform's own code comments). Tools responded with a tool-specific gate, a prefixed variable, or env scrubbing.
4. **Payload incidents are about paths, arguments and unsalted hashes**, not registry hosts. No incident of telemetry leaking private registry hosts was found. Responses: allowlists, salted hashes, categorised hosts, dropping the feature.
5. **`DO_NOT_TRACK` is honored by a minority of the tools reviewed and criticised as conflating "don't collect" with "ask first".**

## Technology Landscape

### Trending (gaining momentum)

| Tool/Pattern | Adoption Signal | Key Benefit | Relevance to Grimoire |
|-------------|----------------|-------------|-------------------|
| Operator-owned OTLP export from an agent CLI, off by default | Claude Code (managed settings), Codex `[otel]`, Copilot CLI v1.0.4, Gemini CLI | Platform team points a fleet at its own collector | Same activation shape as the discussion's proposal |
| Env scrubbing and repo-settings distrust for OTel vars | Claude Code 2.1.128 and 2.1.282 changelog | Stops ambient/repo-supplied endpoints | Hook dispatcher inheritance question |
| Field allowlists instead of redaction | Sanity CLI PR #1975 (open, 2026-10-14), Go upload config | Unlisted fields cannot leak | Payload design |
| Dry-run / log mode to show the payload | gh `GH_TELEMETRY=log`, Next `NEXT_TELEMETRY_DEBUG`, VS Code `--telemetry` | Audit without reading source | Documentation and audit story |

### Established (proven, widely accepted)

| Tool/Pattern | Status | Notes |
|-------------|--------|-------|
| Opt-out with first-run notice and env opt-out | Homebrew, .NET, Next.js, Astro, Gatsby, Turborepo, Prisma | Objections persist 10 years on (Homebrew 2016, .NET 2016) |
| Prompt on first run, then opt-in | Angular CLI, Nuxt | Nuxt also honors `DO_NOT_TRACK` |
| Local-first counters, opt-in upload of an allowlisted subset | Go 1.23 (`local` default) | Small opt-in sample is the cost |
| Documented, per-version field list | .NET Learn page (2026-10), Homebrew `Analytics.md` | Auditable, long |

### Emerging (early but promising)

| Tool/Pattern | Signal | Worth Watching Because |
|-------------|--------|----------------------|
| `DO_NOT_TRACK` convention | HN thread 2026-05-02, 547 points, 168 comments | Cheap fleet-wide kill switch, contested semantics |
| Build-time default-off telemetry | [cli/cli#13260](https://github.com/cli/cli/issues/13260) open; maintainer posted a branch | Distros and enterprises ask for it |
| OTel as telemetry transport for dev tools | .NET 11 Preview 4 moves CLI telemetry to OpenTelemetry | Motivated by AOT, not operator export |

### Declining (losing mindshare)

| Tool/Pattern | Signal | Avoid Because |
|-------------|--------|---------------|
| Google Analytics as CLI sink | Homebrew destroyed its GA data 2023-06-16 | Third-party sink drew the consent objections |
| Calling hashed data "anonymous" | .NET SDK 5 dropped the word (dgl update, 2021) | Unsalted hashes were shown reversible |
| Silent default-on rollout | gh CLI backlash (2026-04) was about the quiet rollout | Release-notes-only disclosure |
| Keeping a scrubbed-but-leaky pipeline | inlang removed all CLI telemetry (2026-10-09) | Raw `args` carried local paths |

## Design Patterns Worth Considering

### 1. How prior tools were received

- **Go (2023, [>18mo]).** Cox's proposal ([Part 1, 2023-02-08](https://research.swtch.com/telemetry-intro)) was default-on. Discussion [golang/go#58409](https://github.com/golang/go/discussions/58409) has 95 comments and 43 upvotes (GitHub GraphQL, 2026-10-22). Critics: affirmative consent, trust in Google, "no reason for a development chain to have any kind of telemetry on by default", GDPR ([The Register, 2023-02-10](https://www.theregister.com/2023/02/10/googles_go_programming_language_telemetry_debate/); [devclass 2024-08-14](https://www.devclass.com/development/2024/08/14/go-123-released-with-telemetry-uploaded-to-google-but-opt-in-after-developer-feedback/1622400)). Supporters (Valsorda, Cox) wanted to debate design, not existence.
  - **Change that resolved it:** [Cox, 2023-02-24](https://research.swtch.com/telemetry-opt-in) switched to opt-in, giving the reason that opt-out "might be used to justify other opt-out systems that are not as careful about what they collect". Design: counters kept locally, weekly upload of only counters listed in a public upload config, no ID, reports stored as local JSON ([#58894](https://github.com/golang/go/issues/58894), [go.dev/doc/telemetry](https://go.dev/doc/telemetry)). Mode `local` is the default: collected on disk, never uploaded ([golang/go#63832](https://github.com/golang/go/issues/63832), 2023-10-30).
  - **Cost:** ~100 uploading users after the gopls launch, ~1,800 weekly after a prompt rolled out to 5% of VS Code Go users; early data had almost no Windows coverage ([Go blog, 2024-09-03](https://go.dev/blog/gotelemetry), **[>18mo]**). Cox's target was "a few hundred thousand" opted-in systems ([opt-in post](https://research.swtch.com/telemetry-opt-in)). Remaining objection: opt-in "will bias the sample set" (devclass).
- **.NET CLI.** Opt-out since 2016. [dotnet/sdk#6145](https://github.com/dotnet/sdk/issues/6145) (opened 2016-05-18, **[>18mo]**, closed without changing the default): no notice at install, per-server env-var burden, healthcare/classified environments ("build-time and local controls" requested), "We are asking for opt-in with explicit consent" (2018). Microsoft answered with a first-run notice and `DOTNET_CLI_TELEMETRY_OPTOUT`. [dgl, 2020-08-23](https://dgl.cx/2020/08/dotnet-sdk-gdpr) showed the payload's `Current Path Hash` and MAC hash were unsalted SHA256 and reproducible; the bounty report was denied; "anonymous" was later dropped from the notice. HN 2022 (Secondary: [hn.svelte.dev](https://hn.svelte.dev/item/33784919), **[>18mo]**) repeated "dark pattern", "tribal knowledge" and installer-telemetry complaints. Today the Learn page lists data points per SDK version and says registry types are reported as categories (Azure, AWS, Google, GitHub, Docker Hub, MCR, Other) ([Learn, updated 2026-10-02](https://learn.microsoft.com/en-us/dotnet/core/tools/telemetry)).
- **Homebrew.** [brew#142](https://github.com/Homebrew/brew/issues/142) (2016-04-25, **[>18mo]**) "Require analytics opt-in rather than make it a blind default": maintainers said it was opt-in for a month, then opt-out; a commenter replied that "the problem with opt-in is that you don't get representative data". Closed next day. [brew#11662](https://github.com/Homebrew/brew/issues/11662) (2021, **[>18mo]**) argued the `uuidgen` user ID is personal data under GDPR. Resolution path: a notice before analytics are enabled, then GA removed and its data destroyed 2023-06-16, EU-hosted InfluxDB ([Homebrew 4.1.0, 2023-07-20](https://brew.sh/2023/07/20/homebrew-4.1.0), **[>18mo]**). Current docs: no user identifier or IP field, 365-day retention, package and non-private tap names collected, the code is authoritative over the page ([docs.brew.sh/Analytics](https://docs.brew.sh/Analytics)).
- **GitHub CLI (2026, most recent).** v2.91.0 made telemetry default-on; The Register's complaint was the quiet rollout, with GitHub replying "enabled by default with a clear opt-out" ([2026-04-22](https://www.theregister.com/2026/04/22/github_opts_all_cli_users/)). Follow-ups: [cli/cli#13260](https://github.com/cli/cli/issues/13260) (build-time default-off, "People may be working on confidential things"), [#13263](https://github.com/cli/cli/issues/13263) (docs insufficient for "informed policy decisions"; asks whether argument values are sent; objects to "review the code"), [#13402](https://github.com/cli/cli/issues/13402) (GHES auto-disable misfires on non-GitHub hosts). Docs: telemetry is not collected when the target is GHES; `GH_TELEMETRY=log` prints the payload ([GitHub docs](https://docs.github.com/en/github-cli/github-cli/github-cli-telemetry)). No default change was reported by the issue pages read.
- **Next.js, Astro, Gatsby.** Opt-out with documented opt-outs and debug modes: Next ([nextjs.org/telemetry](https://nextjs.org/telemetry): `NEXT_TELEMETRY_DEBUG=1` prints to stderr and sends nothing), Astro ([astro.build/telemetry](https://astro.build/telemetry/): lists env vars, paths, git remote and stack traces as not collected; `DEBUG="astro:telemetry"` logs instead of sending), Gatsby ([docs](https://www.gatsbyjs.com/docs/telemetry/): machine ID UUID, one-way hash of cwd or git remote, `GATSBY_TELEMETRY_DEBUG`). Criticism found is blog-level only (Secondary: [Medium](https://florian-martens.medium.com/how-to-stop-your-next-js-app-from-sending-telemetry-data-to-vercel-cf499c773f94): hard-to-find docs, opt-out not opt-in). No substantive GitHub thread located for Gatsby or Astro.
- **VS Code.** `telemetry.telemetryLevel` = `all|error|crash|off`; `code --telemetry` prints a JSON report of possible events; "Developer: Show Telemetry"; stack traces have user paths scrubbed; extension telemetry is not governed by the setting; `off` also disables A/B experimentation; enterprise policy `TelemetryLevel` exists ([telemetry docs](https://code.visualstudio.com/docs/configure/telemetry), [enterprise](https://code.visualstudio.com/docs/enterprise/telemetry), undated). No community thread located.
- **Caddy 0.11 (2018, [>18mo]).** One forum reply asking for opt-in, the exact field list, a sample payload, per-metric consent and an open-source server ([caddy.community](https://caddy.community/t/caddy-0-11-will-have-telemetry-discuss/3610/5)). Outcome not shown on the page.
- **Prompted opt-in.** Angular CLI collects nothing unless the user accepts a first-install prompt ([angular.dev/cli/analytics](https://angular.dev/cli/analytics)); Nuxt "will ask on first time to get permission" ([nuxt/telemetry README](https://github.com/nuxt/telemetry/blob/main/README.md)). No objection threads found for either.

### 2. Enterprise and operator perspective

- **Operator-owned collectors are a shipped pattern for AI-agent CLIs.**
  - Claude Code: admins push `OTEL_EXPORTER_OTLP_*` through managed settings; those OTLP vars override developer-set ones, but exporter selectors (`OTEL_*_EXPORTER`) do not lock ([monitoring docs](https://code.claude.com/docs/en/monitoring-usage)). A request for traces describes "IT configures a central OTLP collector via managed-settings.json" ([anthropics/claude-code#42281](https://github.com/anthropics/claude-code/issues/42281), opened 2026-04-01; shipped in beta 2026-08-17).
  - Copilot CLI: [github/copilot-cli#1911](https://github.com/github/copilot-cli/issues/1911) (2026-03-08) asked for OTel export; shipped in v1.0.4 on 2026-03-28; a user praised "standard OTel env vars, GenAI semantic conventions". The issue itself names developer, not org-wide, use cases.
  - Codex: export disabled by default via an `[otel]` table, `log_user_prompt` false by default ([openai/codex#2103](https://github.com/openai/codex/pull/2103), merged 2025-09-29, description; [Codex docs](https://developers.openai.com/codex/local-config), secondary).
  - Gemini CLI: off by default, `GEMINI_TELEMETRY_*` variables; the `logPrompts` default conflicts across mirrors (Secondary: [last9](https://last9.io/docs/integrations/gemini-cli/)).
- **Platform and vendor guidance.** The OTel project's own post says products "will eventually ask to send logs, traces, and metrics to their own observability stack" and recommends the standard `OTEL_EXPORTER_OTLP_ENDPOINT`; it is silent on default-on or off ([OTel blog, 2026](https://opentelemetry.io/blog/2026/otel-native-by-design/)). .NET 11 Preview 4 notes: the CLI moved to OpenTelemetry exporters because Application Insights blocked NativeAOT, "user-facing behavior is unchanged"; the notes say "opt-in" while keeping the opt-out variable ([release notes](https://github.com/dotnet/core/blob/main/release-notes/11.0/preview/preview4/sdk.md)).
- **Enterprise requests for a switch they control.** gh: [#13260](https://github.com/cli/cli/issues/13260) (build-time default-off for distro packaging). .NET 2016: healthcare/classified users wanted local and build-time controls (dotnet/sdk#6145, **[>18mo]**). Claude Code: server-managed OTel config deployed "company-wide ... a blocker for clean observability at scale" ([#45644](https://github.com/anthropics/claude-code/issues/45644)). VS Code ships a central `TelemetryLevel` policy.
- **Gap:** no thread or blog found about platform teams measuring adoption of an ordinary (non-AI) CLI through OTel. Vendor blogs on "platform scorecards" cover service instrumentation, not CLI usage.

### 3. Practitioner pain with OTel in CLIs

- **Ambient `OTEL_*` triggering unwanted export.**
  - Claude Code set `OTEL_*` for itself; children inherited them. [#35055](https://github.com/anthropics/claude-code/issues/35055) (2026-03-16): user apps (Go, Node, Python, Java) auto-initialised exporters toward `localhost:4317`; managed settings meant users "cannot remove the variables themselves". [#45644](https://github.com/anthropics/claude-code/issues/45644) (2026-04-09, v2.1.97): Docker Compose and OrbStack sent their own telemetry to the company's Honeycomb endpoint. [#55105](https://github.com/anthropics/claude-code/issues/55105) (2026-04-30): an OTLP header carrying a Honeycomb key was reported to trigger board creation on startup. Both earlier issues were closed as duplicates by a bot; the bot's pointer from #45644 to #55105 does not match #55105's title, so the fix claim rests on the changelog.
  - Fix, primary: v2.1.128 "Subprocesses (Bash, hooks, MCP, LSP) no longer inherit `OTEL_*` environment variables" ([CHANGELOG](https://github.com/anthropics/claude-code/blob/main/CHANGELOG.md)); `TRACEPARENT` is still forwarded to Bash/PowerShell subprocesses when tracing is on ([docs](https://code.claude.com/docs/en/monitoring-usage)). v2.1.282: project and local settings now ignore OTel variables that enable export, set the endpoint, or capture content, with a startup notice listing the ignored variables. The doc rationale: "a repository can't use them to turn telemetry on, choose where it goes, or capture content". A repository may still turn a signal off.
  - Docker: [docker/buildx#3381](https://github.com/docker/buildx/issues/3381) (2025-08-26): `OTEL_TRACES_EXPORTER` meant as a bake build argument caused a 10-second delay on a fully cached build; maintainer: "working as designed. Sending OTEL traces is a feature in buildx"; reclassified as a feature request. [buildx#1521](https://github.com/docker/buildx/issues/1521) (2023, **[>18mo]**): `OTEL_EXPORTER_OTLP_ENDPOINT` in a compose `.env` caused a nil-pointer panic; workaround was renaming the variable.
- **Hangs and slow exit.**
  - OTLP's default timeout is 10 s and the SDK default endpoint is `localhost:4317/4318`; `OTEL_TRACES_EXPORTER` defaults to `otlp` ([exporter spec](https://opentelemetry.io/docs/specs/otel/protocol/exporter/), [SDK env spec](https://opentelemetry.io/docs/specs/otel/configuration/sdk-environment-variables/)). That matches the buildx 10 s delay.
  - [moby/buildkit#4616](https://github.com/moby/buildkit/issues/4616) (2024-02, **[>18mo]**): ~20 s stall at the end of a build when the configured OTel endpoint is down; closed 2025-10-29 as "can no longer reproduce".
  - [anthropics/claude-code#30378](https://github.com/anthropics/claude-code/issues/30378) (2026-03-03): subagent processes reported hanging on exit while an exporter retried against HTTP 429 (~300-400 MB each). The quoted log line reads "1P event logging", so whether this was a customer OTLP exporter or first-party event logging is unclear. Closed as a duplicate.
  - Claude Code docs: exit flush "is bounded by a short timeout, so spans can still be dropped"; export errors fail silently unless `CLAUDE_CODE_OTEL_DIAG_STDERR=1` (v2.1.179+) ([Agent SDK observability](https://code.claude.com/docs/en/agent-sdk/observability)). v2.1.275 added a startup warning when `otelHeadersHelper` fails "so sessions that silently export no telemetry are noticed" (CHANGELOG).
- **Gating designs in other CLIs.**
  - Terraform: exports only if `OTEL_TRACES_EXPORTER` is exactly `otlp`, checked by Terraform before calling `autoexport`, because that helper "expect[s] to find an OTLP server on localhost if no environment variables are set"; the comment: "By default we don't export telemetry information at all, since Terraform is a CLI tool and so we don't assume we're running in an environment with a telemetry collector available"; uses a synchronous span processor; interface marked experimental ([telemetry.go](https://github.com/hashicorp/terraform/blob/main/telemetry.go)). [terraform#35444](https://github.com/hashicorp/terraform/issues/35444) (2024-07, **[>18mo]**): `TRACEPARENT` support requested; a maintainer was wary of chasing unstable conventions.
  - Docker CLI: separate variable `DOCKER_CLI_OTEL_EXPORTER_OTLP_ENDPOINT` (or the context's `otel` field), "Only if you've set an environment variable ... will Docker CLI attempt to emit OpenTelemetry metrics" ([docs](https://docs.docker.com/engine/cli/otel/)); export timeout 50 ms ([telemetry.go](https://github.com/docker/cli/blob/master/cli/command/telemetry.go)). First payload scope: command name, duration, status, exit code; a reviewer asked for flag names, not values ([docker/cli#4940](https://github.com/docker/cli/pull/4940), review comment 2024-03-19, **[>18mo]**).
  - gh and Turborepo send from a detached child or background worker (gh `send-telemetry` child; Turborepo buffered worker, 1 s flush); a Windows side effect of gh's detached child flashed a console window (~150 times in 6 days) until `CREATE_NO_WINDOW` shipped in v2.93.0 ([cli/cli#14522](https://github.com/cli/cli/issues/14522), [#13354](https://github.com/cli/cli/issues/13354)). Source: [gh telemetry.go](https://github.com/cli/cli/blob/trunk/internal/telemetry/telemetry.go), [turborepo-telemetry README](https://github.com/vercel/turborepo/blob/main/crates/turborepo-telemetry/README.md).
- **kubectl plugins:** no report found (see negatives).

### 4. Payload sensitivity

| Case | What leaked or was argued | Response |
|------|---------------------------|----------|
| .NET 2020 ([dgl](https://dgl.cx/2020/08/dotnet-sdk-gdpr), [report](https://dgl.cx/2020/08/ms-report.txt), **[>18mo]**) | Unsalted SHA256 of cwd and MAC, reproducible with `sha256sum`; NixOS OS-version string embedded a config hash | Report denied; "anonymous" dropped; docs now list hashed fields per version |
| inlang CLI ([PR #4453](https://github.com/opral/inlang/pull/4453), merged 2026-10-09) | `args` sent to PostHog included `--project /Users/<name>/...`; Sentry reports carried hostname and machine fingerprint | Removed all CLI telemetry instead of scrubbing |
| Sanity CLI ([PR #1975](https://github.com/sanity-io/cli/pull/1975), open, 2026-10-14) | Raw event data could carry filenames and error messages | Event rebuilt from an allowlist of fields and flag names; positional input and `--` args dropped; upload pass also sanitises queued events from older versions; older clients still send the old data |
| Sentry CLI ([PR #1640](https://github.com/getsentry/cli/pull/1640), merged 2026-09-29) | Errors echoed tokens into diagnostics and envelopes | Redaction at the transport boundary; limit: only recognisable token shapes |
| gh CLI (2026) ([#13263](https://github.com/cli/cli/issues/13263)) | Not an incident: request to document whether argument values are sent and how redaction works | Docs list flag names only; GHES targets excluded; `GH_TELEMETRY=log` |
| Astro ([PR #3713](https://github.com/withastro/astro/pull/3713), 2022-06, **[>18mo]**) | Project ID derived from git remote URL | Switched to a hash of git commit data so that "even if the hash is somehow reversed, it is no longer possible to identify the project" (PR text; public repos remain re-derivable per source comment) |

- **Salted vs unsalted:** Next.js hashes the git remote "with random salt data" that is never sent ([project-id.ts](https://github.com/vercel/next.js/blob/canary/packages/next/src/telemetry/project-id.ts)); Turborepo uses a per-machine salt for task and package names (README), while its public docs list those as not collected ([docs](https://turborepo.dev/docs/telemetry)), a page-versus-code mismatch.
- **Allowlists on the receiving side:** Go sends only strings requested by the server's upload config (Ian Lance Taylor in [#58894](https://github.com/golang/go/issues/58894): "By design only strings that are explicitly requested by the telemetry server will be sent"). Numeric IDs in place of strings were rejected as privacy theater and less auditable.
- **Private hosts and names kept out by rule:** Homebrew sends non-private tap names only; gh sends nothing when the target is GHES (and per-request detection is a bug source, [#13402](https://github.com/cli/cli/issues/13402), [PR #14337](https://github.com/cli/cli/pull/14337)); .NET reports registry type, not host. Secondary-adjacent: npm's 2021 public-replica leak of private package names was a database leak, not telemetry ([BleepingComputer](https://www.bleepingcomputer.com/news/security/npm-fixes-private-package-names-leak-serious-authorization-bug/), **[>18mo]**).
- **Content gates in OTel-native tools:** Claude Code redacts prompts, tool details and tool content by default behind `OTEL_LOG_USER_PROMPTS`, `OTEL_LOG_TOOL_DETAILS`, `OTEL_LOG_TOOL_CONTENT`; repository tagging (`OTEL_METRICS_INCLUDE_REPOSITORY`) is opt-in and defaults to false; `user.id` and `user.email` are "always included" ([monitoring docs](https://code.claude.com/docs/en/monitoring-usage)). Codex redacts prompts unless `log_user_prompt = true`.
- **Env-derived signals:** .NET and gh report CI and LLM-agent identity from environment variable presence only, not values (Learn page).

### 5. DO_NOT_TRACK

- **Spec and reception.** Page: "if the variable is set to `1`, disable all tracking", and "Consider making telemetry opt-in rather than opt-out" ([donottrack.sh](https://donottrack.sh/)). Its tool list names tools that have their own opt-outs (.NET, AWS SAM, Azure CLI, Gatsby, Go, Google Cloud SDK, Homebrew, Netlify CLI, Syncthing), not DNT adopters. `consoledonottrack.com` served unrelated spam on 2026-10-22. HN thread (2026-05-02, 547 points, 168 comments, [HN API](https://hacker-news.firebaseio.com/v0/item/47988592.json)); comment content is Secondary via a mirror summary ([hn.nuxt.dev](https://hn.nuxt.dev/item/47988592)):
  - Against: it "suggests the default is CONSENT_TO_TRACK=1" (charles_f); "Opting out of telemetry tells developers that opting us in automatically without consent is OK" (attributed to sneak, described as the original proposer); a tool that honors it may skip an opt-in prompt or disable a crash handler entirely; no enforcement ("no protocol police"); naming ("TRACK" is misleading).
  - For: usage data matters for small projects and opt-in samples are skewed (andybak; Factorio cited); a single standard variable beats per-tool knowledge.
- **Who honors it (primary unless marked).**
  - Yes: Turborepo ([docs](https://turborepo.dev/docs/telemetry), source), Prisma ([docs](https://www.prisma.io/docs/cli/telemetry)), Bun (covers crash-report upload and telemetry, [docs](https://bun.com/docs/runtime/environment-variables)), Nuxt (`isDoNotTrack` in [env.ts](https://github.com/nuxt/telemetry/blob/main/src/utils/env.ts)), GitHub CLI (precedence `GH_TELEMETRY` > `DO_NOT_TRACK` > config, [source](https://github.com/cli/cli/blob/trunk/internal/telemetry/telemetry.go)), Claude Code (changelog fixes for sessions with `DO_NOT_TRACK` set), Hugging Face (Secondary: [docs](https://huggingface.co/docs/huggingface_hub/en/package_reference/environment_variables)).
  - Not found: Homebrew, .NET, Next.js, Astro, Gatsby. Evidence: GitHub code search for `DO_NOT_TRACK` in each repo on 2026-10-22 (may be incomplete), plus a direct read of Astro's [telemetry index.ts](https://github.com/withastro/astro/blob/main/packages/telemetry/src/index.ts), which reads only `ASTRO_TELEMETRY_DISABLED` and `TELEMETRY_DISABLED`.
- **Value parsing differs:** Nuxt accepts `true` and `1`; Prisma's telemetry page documents `1` (Secondary: its environment-variables page reportedly ignores other values); gh's docs show `true`.

## Evidence for and against each design choice

### Activation model

| Choice | Evidence for | Evidence against |
|---|---|---|
| Tool-specific enable switch plus standard `OTEL_*` config (Claude Code, Codex, Terraform's `OTEL_TRACES_EXPORTER=otlp` gate) | Ambient `OTEL_*` is set by other tools and caused unwanted export (#35055, #45644, buildx#3381); Terraform's authors state the SDK helper defaults to `localhost` export; OTel's own blog recommends standard vars for users | Fleet-wide `OTEL_*` (managed settings) still needs the second switch rolled out; Claude Code's managed OTLP vars lock but selectors do not; OTel's SDK default exporter is `otlp` so a library that auto-initialises will export without the switch |
| Prefixed endpoint variable (Docker CLI) | Immune to ambient `OTEL_*` by construction; buildx#1521 workaround was renaming variables | Not standard; Docker's page does not say whether `OTEL_*` is read; operators need per-tool config |
| Opt-out default (.NET, Homebrew, Next, Astro, gh) | Largest samples; Homebrew maintainers: opt-in "you don't get representative data"; Cox's original argument | The same objections recur across a decade (dotnet/sdk#6145, brew#142, gh 2026); enterprise requests for build-time default-off; Go reversed |
| Prompt on first run (Angular, Nuxt) | No objection threads found | Needs an interactive terminal; not applicable to hooks or CI; no adoption figures found |
| Local-first, opt-in upload (Go) | Resolved the main objection; local JSON report is inspectable | ~100 then ~1,800 weekly participants; Windows gap; permanent outreach cost (Go blog 2024) |
| Operator-owned sink (no vendor endpoint) | Removes the "who receives it" objection in .NET/Go/gh threads (all vendor-hosted); matches Claude Code/Codex/Copilot CLI shipping shape | Endpoint can be repo- or env-supplied: Claude Code had to ignore project settings (2.1.282); inherited env carried endpoints to child tools; operator-held data still raises employee-monitoring questions (no thread found either way) |

### Payload contents

| Choice | Evidence for | Evidence against |
|---|---|---|
| Verbatim identifiers (names, refs) | Needed for adoption signal (Homebrew collects package and non-private tap names); sink owned by operator | inlang leaked local paths via raw args; gh#13263 asks whether argument values are sent; Go's allowlist rejects unrequested strings |
| Hashed identifiers | Next/Turborepo salts; Astro commit-hash ID | Unsalted hashes shown reversible (.NET); hashing public names is dictionary-reversible (Astro source comment: public repos re-derivable); salted hashes cannot be joined across machines |
| Categorised values (registry type, host class) | .NET reports registry type; gh categorises hosts (PR #13250 closed in favour of an event closer to the request) | Loses the per-registry signal |
| Allowlist of fields and flag names | Sanity #1975, Go upload config, Docker review comment | Allowlist must be maintained; older clients keep sending old shapes (Sanity) |
| Exclude private targets entirely (GHES, private taps) | Documented in gh and Homebrew | Detection bugs (gh#13402); needs a definition of "private" |
| Identity fields (user, email, device ID) | Useful to operators (Claude Code `user.email`) | Device ID drew "pseudonymous is not anonymous" criticism for gh (Secondary: [korben.info](https://korben.info/en/github-enables-telemetry-by-default-cli.html)); Homebrew 2021 UUID/GDPR issue |

### Documentation and audit story

| Choice | Evidence for | Evidence against |
|---|---|---|
| Per-version documented field list (.NET) | Answers #13263-style questions | Long; Turborepo docs and code disagree; Homebrew states code is authoritative |
| Dry-run or log mode (gh `log`, Next/Gatsby/Turborepo debug, VS Code `--telemetry`) | Auditable without reading source; widespread | gh#13263: "review the code" or log mode alone judged insufficient for policy decisions; Homebrew's `HOMEBREW_ANALYTICS_DEBUG=1` prints the request and still sends it, so debug is not a dry run in every tool ([Analytics.md](https://github.com/Homebrew/brew/blob/main/docs/Analytics.md)) |
| Public change process for fields (Go proposals) | Transparency about changes | Heavy for a small project |
| `DO_NOT_TRACK` honored | One variable works across tools; gh, Nuxt, Turborepo, Bun, Prisma do | HN critique (conflates "don't collect" with "ask first"); value parsing differs; Homebrew, .NET, Next, Astro show no support |

### Delivery

| Choice | Evidence for | Evidence against |
|---|---|---|
| Synchronous, bounded wait (Docker 50 ms export timeout) | No orphan process | Spans dropped when the collector is slow (Claude Code docs); OTLP default timeout is 10 s if not overridden |
| Detached child or background worker (gh, Homebrew, Turborepo) | Parent exits at once; failures silent | Windows console flash (gh#14522); a hung-on-exit report in Claude Code (#30378) suggests the pattern is not automatic with SDK batch processors |
| Silent failure | Never breaks the CLI | Operators cannot tell export is broken (Claude Code added `CLAUDE_CODE_OTEL_DIAG_STDERR`, startup warning for header-helper failure) |

## Key Findings

1. Go's reversal to opt-in came from the discussion itself: "By far the most common suggestion was to make the system opt-in" ([Cox, 2023-02-24](https://research.swtch.com/telemetry-opt-in)) **[>18mo]**; the shipped compromise is local collection plus opt-in upload ([go.dev/doc/telemetry](https://go.dev/doc/telemetry)).
2. Opt-out stayed in .NET, Homebrew and (2026) gh, and enterprise asks for a build-time default-off followed within hours ([cli/cli#13260](https://github.com/cli/cli/issues/13260), opened the day the rollout was reported).
3. Claude Code changed three things in response to env-var problems: scrubbed `OTEL_*` from subprocesses (2.1.128), ignored OTel variables in project settings (2.1.282), and added diagnostics for silent export failure ([CHANGELOG](https://github.com/anthropics/claude-code/blob/main/CHANGELOG.md)).
4. Terraform, Docker CLI and Claude Code each avoid default-on OTel via a gate other than the bare SDK default ([Terraform](https://github.com/hashicorp/terraform/blob/main/telemetry.go), [Docker docs](https://docs.docker.com/engine/cli/otel/), [Claude Code docs](https://code.claude.com/docs/en/monitoring-usage)).
5. Payload failures in 2026 were path and argument leaks through generic event pipelines; the fixes were allowlists or removal ([inlang#4453](https://github.com/opral/inlang/pull/4453), [sanity-io/cli#1975](https://github.com/sanity-io/cli/pull/1975)).
6. The .NET path hash incident shows that a hash of a low-entropy input without a salt was treated as identifying, and the bounty report was denied ([dgl](https://dgl.cx/2020/08/dotnet-sdk-gdpr)) **[>18mo]**.
7. `DO_NOT_TRACK` adoption is partial and its reception is split ([donottrack.sh](https://donottrack.sh/), HN mirror).

## Recommendation

None. The brief for this lane is neutral evidence; the tables above are the input for the activation, payload and documentation decisions. Open items the evidence does not settle: whether any thread treats an operator-owned sink differently from a vendor sink (none found), and whether hook-dispatched processes see `OTEL_*` at all under Claude Code at or above 2.1.128 (changelog says hooks no longer inherit them).

## Negatives (searched, not found)

- Telemetry leaking private registry hosts or private package names: none found. Nearest: path leaks (inlang, .NET) and a non-telemetry npm leak.
- Hang-on-unreachable-collector reports for Terraform or kubectl plugins: none found (Terraform's code comment states the design; its synchronous processor is a code fact, not a reported problem).
- Substantive community threads for Gatsby, Astro and VS Code telemetry reception: none found beyond docs and personal blogs. Caddy's outcome not shown.
- Platform-team blogs or threads on measuring ordinary CLI adoption through OTel: none found (vendor scorecard posts only).
- The Go discussion comments were not read in full; reception rests on The Register, devclass, Cox's posts and issue #58894. No HN thread for Go was located.

## Sources

| Source | Type | Date | Relevance |
|--------|------|------|-----------|
| https://research.swtch.com/telemetry-intro | Blog | 2023-02-08 (>18mo) | Original Go design and anticipated objections |
| https://research.swtch.com/telemetry-opt-in | Blog | 2023-02-24 (>18mo) | Reasons for opt-in, adoption expectations |
| https://github.com/golang/go/discussions/58409 | Thread | 2023-02-08 (>18mo) | 95 comments, 43 upvotes |
| https://github.com/golang/go/issues/58894 | Issue | 2023-03-06 (>18mo) | Opt-in proposal, string allowlist debate |
| https://github.com/golang/go/issues/63832 | Issue | 2023-10-30 (>18mo) | `local` vs `off` modes |
| https://go.dev/doc/telemetry | Docs | undated | Modes, upload rules, storage |
| https://go.dev/blog/gotelemetry | Blog | 2024-09-03 (>18mo) | Opt-in sample size and Windows gap |
| https://www.theregister.com/2023/02/10/googles_go_programming_language_telemetry_debate/ | News | 2023-02-10 (>18mo) | Objections and defenders |
| https://www.devclass.com/development/2024/08/14/go-123-released-with-telemetry-uploaded-to-google-but-opt-in-after-developer-feedback/1622400 | News | 2024-08-14 (>18mo) | Remaining objections |
| https://github.com/dotnet/sdk/issues/6145 | Issue | 2016-05-18 (>18mo) | .NET opt-out objections |
| https://dgl.cx/2020/08/dotnet-sdk-gdpr | Blog | 2020-08-23 (>18mo) | Unsalted hash analysis |
| https://dgl.cx/2020/08/ms-report.txt | Report | 2020-08 (>18mo) | Observed payload fields |
| https://learn.microsoft.com/en-us/dotnet/core/tools/telemetry | Docs | updated 2026-10-02 | Per-version data points, registry categories, env-var detection |
| https://github.com/dotnet/core/blob/main/release-notes/11.0/preview/preview4/sdk.md | Release notes | 2026 | CLI telemetry moves to OpenTelemetry |
| https://hn.svelte.dev/item/33784919 | HN mirror | ~2022-11 (>18mo) | .NET telemetry complaints (Secondary) |
| https://github.com/Homebrew/brew/issues/142 | Issue | 2016-04-25 (>18mo) | Opt-in request and rebuttal |
| https://github.com/Homebrew/brew/issues/11662 | Issue | 2021-07-06 (>18mo) | GDPR/UUID argument |
| https://brew.sh/2023/07/20/homebrew-4.1.0 | Blog | 2023-07-20 (>18mo) | GA data destroyed, InfluxDB EU |
| https://docs.brew.sh/Analytics | Docs | undated | Fields, retention, opt-out, debug |
| https://nextjs.org/telemetry | Docs | undated | Fields, debug mode, opt-out |
| https://github.com/vercel/next.js/blob/canary/packages/next/src/telemetry/project-id.ts | Source | current | Salted git-remote hash |
| https://astro.build/telemetry/ | Docs | updated 2026-07-13 | Not-collected list, agent detection |
| https://github.com/withastro/astro/pull/3713 | PR | 2022-06 (>18mo) | Project ID derivation change |
| https://github.com/withastro/astro/blob/main/packages/telemetry/src/index.ts | Source | current | No `DO_NOT_TRACK` |
| https://www.gatsbyjs.com/docs/telemetry/ | Docs | undated | Fields, hashing, debug |
| https://code.visualstudio.com/docs/configure/telemetry | Docs | undated | Levels, `--telemetry`, extensions |
| https://code.visualstudio.com/docs/enterprise/telemetry | Docs | undated | Central policy |
| https://caddy.community/t/caddy-0-11-will-have-telemetry-discuss/3610/5 | Forum | 2018-03-31 (>18mo) | Early opt-in request |
| https://angular.dev/cli/analytics | Docs | undated | First-install prompt |
| https://github.com/nuxt/telemetry/blob/main/README.md | Docs | current | Prompt plus `DO_NOT_TRACK` |
| https://www.theregister.com/2026/04/22/github_opts_all_cli_users/ | News | 2026-04-22 | gh default-on rollout |
| https://docs.github.com/en/github-cli/github-cli/github-cli-telemetry | Docs | 2026 | Fields, GHES exclusion, log mode |
| https://github.com/cli/cli/issues/13260 , /13263 , /13402 , /14522 , /13354 | Issues | 2026-04 to 2026-09 | Enterprise ask, docs critique, GHES bug, Windows detach |
| https://github.com/cli/cli/blob/trunk/internal/telemetry/telemetry.go | Source | current | Precedence, sampling, detached child |
| https://code.claude.com/docs/en/monitoring-usage | Docs | current | Gates, managed settings, content redaction, project-settings rule |
| https://code.claude.com/docs/en/agent-sdk/observability | Docs | current | Bounded flush, silent failure |
| https://github.com/anthropics/claude-code/blob/main/CHANGELOG.md | Changelog | current | 2.1.128, 2.1.275, 2.1.282 entries |
| https://github.com/anthropics/claude-code/issues/35055 , /45644 , /55105 , /30378 , /42281 | Issues | 2026-03 to 2026-08 | Leakage, hang, central collector |
| https://github.com/github/copilot-cli/issues/1911 | Issue | 2026-03-08 | Copilot CLI OTel request and ship |
| https://github.com/openai/codex/pull/2103 | PR | opened 2025-08-09, merged 2025-09-29 | Codex `[otel]` defaults (PR description) |
| https://github.com/hashicorp/terraform/blob/main/telemetry.go | Source | current | Terraform gate and rationale |
| https://github.com/hashicorp/terraform/issues/35444 | Issue | 2024-07-10 (>18mo) | `TRACEPARENT` request |
| https://docs.docker.com/engine/cli/otel/ | Docs | current | Docker CLI opt-in endpoint |
| https://github.com/docker/cli/blob/master/cli/command/telemetry.go | Source | current | 50 ms export timeout |
| https://github.com/docker/cli/pull/4940 | PR | 2024-03-13 (>18mo) | First payload, flag-name review comment |
| https://github.com/docker/buildx/issues/3381 | Issue | 2025-08-26 | 10 s delay from ambient var |
| https://github.com/docker/buildx/issues/1521 | Issue | 2023-01-16 (>18mo) | Panic, rename workaround |
| https://github.com/moby/buildkit/issues/4616 | Issue | 2024-02-02 (>18mo) | Stall when endpoint absent |
| https://opentelemetry.io/docs/specs/otel/protocol/exporter/ | Spec | current | Default timeout and endpoints |
| https://opentelemetry.io/docs/specs/otel/configuration/sdk-environment-variables/ | Spec | current | `OTEL_SDK_DISABLED`, default exporter |
| https://opentelemetry.io/blog/2026/otel-native-by-design/ | Blog | 2026 | Standard vars for customer export |
| https://github.com/opral/inlang/pull/4453 | PR | 2026-10-09 | Telemetry removed after path leak |
| https://github.com/sanity-io/cli/pull/1975 | PR | 2026-10-14 | Allowlist redaction |
| https://github.com/getsentry/cli/pull/1640 | PR | 2026-09-28 | Transport-boundary redaction |
| https://turborepo.dev/docs/telemetry | Docs | current | Opt-outs incl. `DO_NOT_TRACK` |
| https://github.com/vercel/turborepo/blob/main/crates/turborepo-telemetry/README.md | Source docs | current | Salt, background worker |
| https://www.prisma.io/docs/cli/telemetry | Docs | current | `DO_NOT_TRACK` |
| https://bun.com/docs/runtime/environment-variables | Docs | current | `DO_NOT_TRACK` covers crash reports |
| https://github.com/nuxt/telemetry/blob/main/src/utils/env.ts | Source | current | `isDoNotTrack` |
| https://huggingface.co/docs/huggingface_hub/en/package_reference/environment_variables | Docs | current | `DO_NOT_TRACK` (Secondary) |
| https://donottrack.sh/ | Spec page | undated | Proposal text, tool list |
| https://hacker-news.firebaseio.com/v0/item/47988592.json | HN API | 2026-05-02 | Thread metadata |
| https://hn.nuxt.dev/item/47988592 | HN mirror | 2026-05 | Comment arguments (Secondary) |
| https://www.bleepingcomputer.com/news/security/npm-fixes-private-package-names-leak-serious-authorization-bug/ | News | 2021-11 (>18mo) | Non-telemetry private-name leak, for contrast |
