# Research: Hook / lifecycle-event surfaces across all 17 grim clients

## Metadata

**Date:** 2026-08-14
**Domain:** integration | packaging | security
**Triggered by:** maintainer feature request — support hooks as a distributable artifact kind
**Expires:** 2026-11-14 (**short deliberately** — three clients shipped or reworked hooks
within 90 days of this survey; two more have open reliability bugs against the surface)
**Method:** one Sonnet 5 researcher per client, primary sources only (vendor docs, vendor
changelogs, vendor source, official issue trackers), every claim URL-anchored and fetch-dated.
Undocumented behaviour recorded as `NOT DOCUMENTED` rather than guessed.
**Primary evidence:** [`hooks_vendor_reports/`](./hooks_vendor_reports/) — the 17 full
per-client reports (quote-heavy, with per-file Sources tables). The brief they answered is
[`hooks_vendor_reports/_BRIEF.md`](./hooks_vendor_reports/_BRIEF.md).
**Companion:** [`research_hooks_trampoline.md`](./research_hooks_trampoline.md) — the grim-side
machinery analysis and the open decisions.
**Supersedes:** [`research_ide_hooks.md`](./research_ide_hooks.md) (2026-06-03) for all vendor
facts — that survey covers Windsurf / Continue / Aider (not grim clients) and omits 9 of the
17. Its portability and security *analysis* is still sound.

## Direct answer

**15 of 17 clients have a hook mechanism. Only Warp and Zed have none.** That is a sharp change
from the June 2026 survey, and it inverts the framing: hooks are no longer a
Claude-Code-plus-a-few feature, they are near-universal, and the interesting question is not
*whether* a client has hooks but *what shape its install surface takes*.

Three install shapes cover every client, and grim already ships machinery for all three.

## Master matrix

`Shell` = a hook is an external command/executable the client spawns.
`JS` = a hook is an in-process JavaScript/TypeScript function the client imports.

| Client | Hooks | Kind | Install surface | Events | Can block? | Native identity | Reload |
|---|---|---|---|---|---|---|---|
| **claude** | yes, mature | Shell (+http/prompt/agent/mcp_tool) | splice `settings.json` (4 scopes) | 30+ | yes, rich | **none** | hot |
| **copilot** | yes, mature | Shell (+http/prompt) | **own a file** — `.github/hooks/*.json`, `~/.copilot/hooks/` | 14 (CLI) / 8 (VS Code) | yes | **none** | CLI: restart · VS Code: hot |
| **cursor** | yes | Shell (+prompt) | splice `.cursor/hooks.json` | ~20 | **deny only** (allow/ask staff-confirmed broken) | **none** | hot |
| **gemini** | yes | Shell | splice `.gemini/settings.json` | 11 | yes | `name:command` fingerprint | partial |
| **droid** | yes | Shell | splice `hooks` key in `.factory/settings.json` | 9 | yes | **none** | restart (snapshot) |
| **antigravity** | yes | Shell | splice `hooks.json` (named map) | 5 | yes, richest enum | **the map key** | NOT DOCUMENTED |
| **kiro** | yes | Shell + `agent` prompt | **own a file** — `.kiro/hooks/*.json`, `~/.kiro/hooks/` | 10 | exit-code only | `name` (not unique-enforced) | NOT DOCUMENTED |
| **goose** | yes | Shell | **own a dir** — `~/.agents/plugins/<name>/hooks/hooks.json` | 11 | block-only, binary | plugin **directory** | NOT DOCUMENTED |
| **cline** | yes | Shell | **own a file, filename == event** — `.clinerules/hooks/<Event>`, `~/Documents/Cline/Hooks/` | 9 | yes | **the filename** | n/a |
| **junie** | yes, **EAP** | Shell | splice `.junie/config.json` | 7 | yes | **none** | NOT DOCUMENTED |
| **amp** | partial | **JS** plugin + a legacy shell `delegate` | codegen `.ts` plugin, or splice `amp.permissions` array | 5 (JS) / 1 (delegate) | yes | file path / none | **no hot reload** |
| **kilo** | yes | **JS** (OpenCode runtime, ported) | codegen `.ts` into `.kilo/plugin/` | ~20 + event bus | yes (throw / mutate) | plugin `id` or filename | restart |
| **openclaw** | yes | **JS** (in-process), file-discovered | **own a dir** — `HOOK.md` + generated `handler.js` | coarse lifecycle + typed plugin hooks | typed hooks only | hook name | NOT DOCUMENTED |
| **codex** | yes, mature | Shell | **own a file** — `.codex/hooks.json` / `$CODEX_HOME/hooks.json` (or splice `[hooks]` in `config.toml`) | 10 | yes | trust identity by hash | n/a |
| **opencode** | yes | **JS** only | codegen `.{ts,js}` into `.opencode/plugin(s)/` | 3 named + event bus | throw / `output.status` | file path | restart |
| **warp** | **none** | — | — | 0 | — | — | — |
| **zed** | **none** (agent) | — | — | 0 (agent) | — | — | — |

**Retracted 2026-08-14 — this claim was wrong, and the code is fine.** The paragraph below
asserted that `vendor_codex.rs:106` records Codex hooks as "rejected upstream" and contradicts
line 20 of the same file. Verified against `03e59b0`: it does not. The comment reads *"Codex has
no path-scoped instruction mechanism (no globs/applyTo anywhere; hooks cannot supply file-aware
context). Rules are declined"* — the hooks clause is a parenthetical explaining why hooks do not
rescue **rules**, not a claim about hook support, and it agrees with the module doc at
`vendor_codex.rs:20-22`. **There is nothing stale to fix in that file.** The rest of the
paragraph — that Codex ships a mature hook engine — stands, and was never in dispute.

~~**Codex correction — grim's own code is stale.**~~ `vendor_codex.rs:106` records that Codex
"hooks [were] rejected upstream", which is no longer true: Codex ships a mature hook engine
(`codex-rs/hooks/`), documented at `developers.openai.com/codex/hooks` (→ `learn.chatgpt.com/
docs/hooks`) and in the repo's `docs/config.md` "Lifecycle hooks" section. `vendor_codex.rs:20`
already cites `openai/codex#20692` about hooks accepting `additionalContext`, so the two
comments in one file disagree — fix the stale one. Codex is also the **only client offering
both install shapes** (a standalone `hooks.json` *and* an inline `[hooks]` TOML table), whose
"trust identity" the discovery code deliberately converges; hook entries **append** across
every trusted layer (`append_hook_events`), the opposite of ordinary Codex config precedence.
Admin controls: `allow_managed_hooks_only = true` in `requirements.toml` only, and
`[features] hooks = false` as a kill switch.

**OpenCode has no config-level hook surface at all** — confirmed by reading the authoritative
config schema (`packages/core/src/v1/config/config.ts`); the plugin system is the only
mechanism. Discovery glob is `{plugin,plugins}/*.{ts,js}` — **both singular and plural**
directory names load, non-recursive. Denying a tool call has no first-class field: the shim must
`throw`, which surfaces as a tool *error*, not a clean denial. Only `permission.ask` has a real
three-state `output.status`.

## The three install shapes — and the grim mechanism each maps onto

This is the load-bearing result of the survey.

| Shape | Clients | grim mechanism | New machinery |
|---|---|---|---|
| **Own a whole file / directory** (the client discovers by directory glob or filename) | copilot, **codex**, kiro, goose, cline, openclaw | the **rule/skill materialization** path: write bytes, hash the footprint, integrity-gate, uninstall deletes | **none** |
| **Splice a shared user-owned config** | claude, cursor, gemini, droid, antigravity, junie | the **MCP entry / opencode-`instructions`** path: `json_splice` + `sync_config` + entry-typed `ClientOutput` | extend splice to nested + object array elements |
| **Codegen a JS/TS module that shells out** | amp, kilo, **opencode**, openclaw | the **generated-doc** path: `RenderedDoc`, `generated: true`, provenance header, self-heal | a per-vendor shim template |

The first bucket is the surprise, and it is **six** clients — over a third of the roster — where
hooks need *no config editing at all*. Cline is the extreme case: the hook **is** a file whose
**filename is the event name**, validated against a hardcoded allow-list
(`apps/vscode/src/core/hooks/utils.ts` — `TaskStart`, `TaskResume`, `TaskCancel`,
`TaskComplete`, `PreToolUse`, `PostToolUse`, `UserPromptSubmit`, `Notification`, `PreCompact`).
Goose's official install instructions are literally `mkdir`/`cp`/`chmod` into the **same
`~/.agents/` pool grim already writes skills into**.

**Corollary:** the hook-capable client set is **not** a subset of the rule/agent-capable set.
Droid, Goose, Cline and Kilo decline rules, agents *and* MCP in grim today, yet all four have
hooks. The v1 client list cannot be lifted from `docs/src/clients.md`.

## Claude Code's contract is the de facto standard

Not asserted — observed, from five independent researchers:

| Evidence | Source |
|---|---|
| Cursor ships an opt-in importer that reads `.claude/settings.json`, maps Claude's PascalCase event names to its own camelCase, and sniffs both response dialects on one wire | `cursor.com/docs/reference/third-party-hooks` |
| Copilot's VS Code agent hooks read `.claude/settings.json` and are "explicitly built to the same wire format as Claude Code/Copilot CLI" | VS Code agent-hooks docs |
| Gemini CLI exports a `CLAUDE_PROJECT_DIR` alias env var, verbatim "for compatibility" with Claude Code scripts | Gemini hooks reference |
| Goose's own PR #9304 admits following "precedent for adopting Claude Code's hook conventions" | `aaif-goose/goose` PR #9304 |
| **Codex's own internal Rust type is named `ClaudeHooksEngine`**, and its open umbrella tracker is `openai/codex#21753` — *"Full Claude Code Hook Parity (29+)"* (since 2026-05-08) | `codex-rs/hooks/`, GitHub |
| Junie, Droid, Goose, Gemini, Cline, Codex all reuse `{Event: [{matcher, hooks:[{type:"command",command,timeout}]}]}` plus `decision` / `additionalContext` / `continue` / `stopReason` / `hookSpecificOutput`, PascalCase event names, and exit-2-blocks | per-client reports |

**Design consequence:** grim's canonical hook schema should *be* Claude's shape with Claude's
PascalCase event names, not a neutral invention. Translation then collapses to an event-name
map plus small key tweaks for the majority of targets, and an author who knows Claude hooks
already knows grim hooks.

## Canonical event core

Present, with the same semantics, in enough clients to be portable:

| Canonical (Claude spelling) | claude | copilot | cursor | gemini | droid | kiro | goose | cline | junie | antigravity |
|---|---|---|---|---|---|---|---|---|---|---|
| `SessionStart` | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓* | ✓ | ✗ |
| `UserPromptSubmit` | ✓ | ✓ | ✓† | ✗ | ✓ | ✓ | ✓ | ✓ | ✓ | ✗ |
| `PreToolUse` | ✓ | ✓ | ✓ | ✓‡ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ |
| `PostToolUse` | ✓ | ✓ | ✓ | ✓‡ | ✓ | ✓ | ✓ | ✓ | ✗ | ✓ |
| `Stop` | ✓ | ✓ | ✓ | ✓‡ | ✓ | ✓ | ✓ | ✓* | ✓ | ✓ |
| `SessionEnd` | ✓ | ✓ | ✓ | ✓ | ✓ | ✗ | ✓ | ✗ | ✓ | ✗ |
| `PreCompact` | ✓ | ✓ | ✓ | ✓‡ | ✓ | ✗ | ✗ | ✓ | ✗ | ✗ |

`*` Cline spells them `TaskStart` / `TaskComplete`. `†` Cursor: `beforeSubmitPrompt`.
`‡` Gemini: `BeforeTool` / `AfterTool` / `AfterAgent` / `PreCompress`.

`PreToolUse` and `Stop` are the only two present in **all 15** hook-capable clients.
`PreToolUse` + `PostToolUse` + `SessionStart` + `Stop` is a defensible portable core; anything
beyond that should be authored as a native per-vendor event.

## The response contract is where portability actually dies

Payloads are informational and a superset is harmless. Responses are semantic, and they
diverge in ways that change behaviour:

| Divergence | Detail |
|---|---|
| **Failure direction is opposite** | Claude, Cursor, Gemini, Goose, Junie, Droid fail **open** on an unexpected non-zero exit. Copilot's `preToolUse` fails **closed** on error (but still fails open on *timeout*). Cursor adds an opt-in `failClosed` per hook. The same unchanged hook therefore has opposite behaviour when it crashes. |
| **Response power varies by an order of magnitude** | Antigravity: `allow`/`deny`/`ask`/`force_ask`/`deny_unless_prior_grant`. Claude: `permissionDecision` + `updatedInput` + `additionalContext` + `continue`/`stopReason` + `systemMessage`. Goose: **block or nothing**, binary. Kiro: **no JSON response object at all** — exit code plus stdout-as-context. |
| **Verdict location moves per event** | Within Claude alone: `hookSpecificOutput.permissionDecision` for one event, top-level `decision` for another, `hookSpecificOutput.decision.behavior` for a third — and `hookSpecificOutput.hookEventName` must echo the firing event. A generic dispatcher needs a **per-event response table**, not one pass-through. |
| **The schema is version-sensitive** | Claude's `continueOnBlock` default flipped at v2.1.210. A response grim emits once can change meaning under a client upgrade grim does not control. |
| **`allow` may be a lie** | Cursor: `allow`/`ask` verdicts from hooks are **not enforced** — staff-confirmed open bug, only `deny` works. Do not design around `ask`. |
| **Multi-hook arbitration is undefined** | Claude runs hooks in parallel and `updatedInput` is last-process-to-exit-wins. Kiro/Goose short-circuit on first deny for pre-tool only. Most clients: ordering `NOT DOCUMENTED`. |

## Security posture per client — and what it means for grim

| Client | Client-side gate on a repo-provided hook |
|---|---|
| **gemini** | Strongest: three-tier trust (system/user/project), project hooks fingerprinted by `name:command` in `trusted_hooks.json`, **re-prompts when the fingerprint changes**. Explicit "arbitrary code … with your user privileges" warning. |
| **junie** | Verbatim: *"Project-local hooks from `<project-root>/.junie/config.json` are ignored by default for safety."* Project-trust markers, OS-keychain-backed. Headless/CI currently **skips** the trust gate. |
| **copilot** | Folder-trust dialog gates repo hooks; `-p` mode locks repo hooks behind opt-in env vars. Vendor: *"Hooks should be treated as security-sensitive code."* Enterprise policy hooks are root-owned. |
| **droid** | Explicit warnings, config **snapshot at startup** with external-modification warning, global `hooksDisabled` kill switch, `allowManagedHooksOnly`. |
| **claude** | Weakest relative to its power: workspace trust does **not** block settings-file hooks in most session types; no hooks-specific "arbitrary code" warning in the security docs. |
| **cursor, kiro, antigravity, amp, goose, cline, kilo** | No hook-specific consent step found. Goose documents "run trusted hooks only" prose; Amp states *"Amp does not ask for approval before running tools"*; Kilo auto-executes local `.kilo/plugin/*.ts` at startup with no prompt. |

**Conclusion for grim:** the clients will not gate this for us. On the majority of targets an
installed hook simply runs. grim must own consent itself — and Gemini's fingerprint-and-
re-approve model is the one to copy, since it is both the strongest observed and the one whose
friction grim will otherwise trip over in CI.

## Cross-cutting: the industry just declined to standardize hooks

**Agent Plugins 1.0** (`agent-plugins.org`, v1.0.0, published **2026-08-06** by GitHub, AWS,
Cursor, Microsoft, OpenAI, Vercel and Google) standardizes skills and MCP packaging and
**explicitly places hooks outside its portable core**, treating them as a Copilot-specific
extension. Found independently by the Copilot and Goose researchers.

**Corrected 2026-08-14 (design-panel gap check).** The original wording here read "there is no
open spec to defer to, so a portable hook layer is an **uncontested gap**". The first clause is
right; the second was wrong, and the distinction matters:

- **No open *spec*** — confirmed and re-verified today. Agent Plugins 1.0 still excludes hooks
  (no companion proposal found), ACP has no hook/middleware shape, and MCP's three official
  extensions as of 2026-07-28 (Tasks, MCP Apps, EMA) are none of them hook-shaped. There is
  genuinely nothing to align the canonical envelope with.
- **But the niche is *not* uncontested.** `sondera-ai/sondera-coding-agent-hooks` (MIT, ~222
  stars, created 2026-02-27, last push 2026-08-11 — verified live via `gh api`) ships per-vendor
  hook adapters for **10 of the 15** hook-capable clients here (Claude Code, Cursor, Copilot,
  Gemini CLI, Antigravity, Codex, Hermes, OpenCode, OpenHands, VS Code). Rust workspace,
  `crates/{hooks,policy,guardrails,harness,schema,trajectory}` plus one `apps/sondera` binary.
  Each adapter normalizes its vendor's event and forwards it over gRPC to `sondera serve`, which
  combines signature scanning with **Cedar** policy evaluation and returns a canonical
  Allow/Deny/Escalate. Presented at [un]prompted 2026 and Black Hat Arsenal 2026 ("Hooking
  Coding Agents with the Cedar Policy Language", Matt Maisel), plus an ICML 2026 workshop paper.

**Why the contrast is useful rather than just a correction.** Sondera made the **opposite call on
the two hardest questions in this design**, and its reasons are legible: a **resident daemon**
(where grim chose a per-event launcher) that **fails closed** when unreachable (where grim chose
fail-open). Both are defensible *from each product's purpose* — Sondera is a security-enforcement
product, so an unreachable policy engine must block; grim is a package manager, so a missing grim
must not break the user's agent. That is validation-by-contrast for grim's fail-open default, and
it is also proof that the daemon option is practically viable, not merely theoretical — which
keeps it a credible escape hatch if measurement demands one.

**Follow-up, not done here:** Sondera belongs in the comparable-tools tracking that
`product-context.md` routes to `research_promotion_positioning.md` › "Competitive landscape".
Adding it there is a positioning edit outside this survey's scope.

Separately and unrelated to hooks: a seven-vendor packaging standard for *skills* that shipped
eight days before this survey warrants its own review against grimoire's positioning and
`product-context.md`.

Goose's docs separately call their own format "Open Plugins", for which no independent spec
was found. It is **not** Agent Plugins; do not conflate them.

## Side findings for other subsystems (not hooks — verify before acting)

Each of these came out of a hooks researcher reading a vendor's current docs, and each
contradicts something grim ships today. None is verified against grim's code by this survey.

1. **kilo global root.** Every current Kilo doc puts global config at **`~/.config/kilo/`**;
   grim assumes `~/.kilo` (with `$XDG_CONFIG_HOME/kilo` OR-ed for detection only). Kilo was
   also rebuilt on OpenCode's runtime on 2026-04-02 (cache path is literally
   `~/.cache/opencode/`, engine key `"engines":{"opencode":"^X.Y.Z"}`), and the `.opencode`
   config fallback was **removed** 2026-06-24 while `.kilocode` is still read.
2. **openclaw env vars — a watchlist item that can be closed.** grim's watchlist records
   `$OPENCLAW_HOME` as "referenced in OpenClaw material but never defined on any page fetched".
   **It is defined** — `docs/help/environment.md` in `openclaw/openclaw` — and it has the
   **`GEMINI_CLI_HOME` shape, not the `CLAUDE_CONFIG_DIR` shape**: it replaces `$HOME` /
   `os.homedir()` wholesale for every OpenClaw path default. Precedence `OPENCLAW_HOME` >
   `$HOME` > `USERPROFILE` > `os.homedir()`, with explicit `OPENCLAW_STATE_DIR` /
   `OPENCLAW_CONFIG_PATH` still winning over it. It was almost certainly missed because
   `docs.openclaw.ai/*` returns **HTTP 403** to automated fetches; the researcher read the docs
   source out of the GitHub tree instead (`gh api .../contents/<path>` + base64), which is the
   technique to reuse for this vendor. Also: config is `~/.openclaw/openclaw.json` in **JSON5**
   (comments + trailing commas — grim's JSONC-tolerant scanner may or may not cover JSON5),
   with `OPENCLAW_INCLUDE_ROOTS` for `$include` directives. **This affects where grim installs
   OpenClaw skills today**, not just hooks.
3. **cline global path.** Cline's global File Hooks live in **`~/Documents/Cline/Hooks/`**, not
   under `~/.cline`. Worth checking whether Cline's global *skills* root is likewise not
   `~/.cline/skills/`.
4. **warp shared pool.** Warp's current docs list `~/.warp/skills/` **and** `~/.agents/skills/`
   as both scanned, with no opt-in gate visible — grim gates the pool for Warp behind
   `shared_skills`. grim's stance ("membership is about what a client *reads*, not where grim
   *writes*") may already cover this; re-read before changing anything.
5. **junie paths.** `.junie/guidelines.md` is deprecated legacy; current is `.junie/AGENTS.md`.
   No `.junie/rules/*.md` was found in current docs — grim installs rules there.
6. **goose ownership moved.** Block donated Goose to the Agentic AI Foundation (Linux
   Foundation) on 2026-04-07. Canonical repo `github.com/aaif-goose/goose`, docs
   `goose-docs.ai`; `block.github.io/goose` now redirects. Windows config path still contains
   the legacy `Block\goose` segment.
7. **gemini client in flux.** Google stopped serving free/Pro hosted inference for Gemini CLI
   on 2026-06-18, positioning Antigravity CLI as successor ("retains … Hooks"). OSS repo is
   alive and shipping nightlies; enterprise licensees unaffected.

## Open reliability bugs that would land on grim

| Client | Bug | Impact on a grim hook install |
|---|---|---|
| **droid** | `Factory-AI/factory#3` (open) — the documented-primary `.factory/hooks.json` is silently never read at project scope; only the `hooks` key in `settings.json` fires | grim must target `settings.json`, i.e. the splice path, not the own-a-file path |
| **cursor** | `allow`/`ask` verdicts not enforced (staff-confirmed, open) | only `deny` is real on Cursor |
| **kiro** | `#7375` (open) — the **IDE** delivers `{}` on stdin and puts the payload in a `USER_PROMPT` env var; the CLI is correct | a stdin-reading trampoline gets an empty envelope in the Kiro IDE |
| **kiro** | `#7915` — `timeout` override broken on Windows | |
| **kiro** | `#9007` (open) — Kiro's own plugin system ("Powers") cannot deliver hooks; direct file-write is the only channel | file-write is the *only* option, which suits grim |
| **copilot** | `#2893` (open) — parallel tool calls dispatch `preToolUse` hooks serially with gaps | |
| **antigravity** | five dated hook bugs fixed May–Aug 2026 (Stop-hooks hanging the agent forever; bad configs silently ignored, fixed 2026-08-07) | surface is still hardening |
| **warp** | `#7834`, `#6857`, `#12868` — three open, maintainer-silent requests for exactly this feature; #7834 proposes `~/.warp/hooks.yaml` + JSON-on-stdin, modeled on Claude Code | if it ships, it will likely be trampoline-shaped |

## Trampoline viability per client

| Verdict | Clients |
|---|---|
| **Direct** — one generic `command` string/argv is a first-class hook value | claude, copilot, **codex**, cursor, gemini, droid, kiro (`command` action only), goose, junie, antigravity, cline — **11 clients** |
| **One layer removed** — no shell hook type; grim must generate a JS/TS shim that shells out | amp, kilo, **opencode**, openclaw |
| **Not viable** — no target exists | warp, zed |
| **Never viable, by nature** | Kiro's `agent` action and Cursor's/Claude's `prompt`/`agent` handler types: the *LLM call is the handler*, there is no process to stand in for. Claude's `http` type needs a listener, not a spawned CLI. Model these as native-only, not as portable hooks. |

Surface-specific blocker worth naming: **Copilot's cloud agent** reads `.github/hooks/*.json`
only from the **default branch**, in an ephemeral firewalled sandbox — the `grim` binary is not
present there unless `copilot-setup-steps.yml` installs it first. That is the one surface where
the trampoline model genuinely cannot reach.

## Durable search terms

`claude code hooks reference hookSpecificOutput` · `cursor hooks.json third-party-hooks
importer` · `copilot .github/hooks agent hooks preview` · `gemini cli hooks trusted_hooks.json
getHookKey` · `droid hooks settings.json Factory-AI/factory#3` · `kiro .kiro/hooks agent hooks
v1` · `goose lifecycle hooks open plugins aaif-goose` · `cline VALID_HOOK_TYPES .clinerules
hooks` · `junie CLI hooks EAP config.json` · `antigravity hooks.json PreInvocation` ·
`amp plugin amp.on permissions delegate` · `kilo plugin opencode runtime` · `openclaw HOOK.md
handler.js typed plugin hooks` · `agent-plugins.org hooks outside portable core`
