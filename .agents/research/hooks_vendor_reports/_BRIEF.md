# Research brief — native hook / lifecycle-event mechanism of ONE AI client

## Why we need this

`grimoire` is a package manager (binary `grim`) that installs AI-agent configuration —
skills, rules, agents, MCP server descriptors — into **17 different AI coding clients** by
writing each client's *native* files (and splicing its native JSON/TOML config files
in place, preserving every byte outside the managed member).

We are evaluating a **new artifact kind: `hook`** — a portable, registry-distributed,
event-driven hook that grim materializes per client. To design the portable schema we need
the *exact* native contract of every client. Your job is ONE client.

## Evidence rules (strict)

- **Primary sources only** as fact: the vendor's official docs, official changelog /
  release notes, the client's own public source, official GitHub/GitLab issues & PRs.
- **Fetch the pages.** Do not answer from recollection. Every claim carries a URL.
- Third-party blogs / Reddit / unofficial wikis: allowed as *leads*, must be labelled
  `[unofficial]`, never stated as fact.
- Today is **2026-08-14**. Record the date you fetched each source and every version gate
  ("since v1.4", "experimental", "behind feature flag X", "beta", "deprecated").
- **Exact strings matter more than prose.** Event names, JSON keys, env var names, file
  paths, exit-code meanings, schema shapes. If something is undocumented, write
  `NOT DOCUMENTED` — never guess. A wrong exact string is worse than an admitted gap.
- If the client has **no** hook mechanism, *prove the absence*: name the docs pages/search
  you checked, plus any upstream issue/PR requesting or rejecting hooks (number, title,
  state, date).

## Scope boundary

We mean **hooks**: client-invoked, deterministic execution of *user-supplied code* at
lifecycle/tool events. Adjacent things that are NOT hooks (mention only to disambiguate,
clearly labelled): slash commands, skills, subagents, MCP servers, rules/instructions,
custom tools, LSP/formatter integration, git hooks.

**Also in scope** if the client implements the same idea under another name: "plugins" with
event handlers, "automations", "notifications" (e.g. a `notify` command), "agent hooks",
"lifecycle events", "triggers", "event listeners", "middleware", "guardrails/policy hooks".

## Questions — answer in this order

1. **Existence & name.** Does it exist? What does the vendor call it? Stable, beta,
   experimental, or flag-gated? Since which version? Any deprecation notice?
2. **Config location(s).** Exact file paths for **project** scope and **global/user**
   scope. Format (JSON / JSONC / TOML / YAML / JS / TS). Any env var that relocates it.
   Is there also a *directory* convention (e.g. `<root>/hooks/*.json`, `plugin/*.ts`)
   that gets auto-discovered? Do multiple sources **merge** or does one win?
3. **Config schema — verbatim.** The exact key path and shape. Critically: is the hook
   collection a **named map** (`{"hooks": {"<event>": {...}}}`) or an **array** of hook
   entries? Include a real example copied from the docs. Note matcher/filter syntax (tool
   name globs, regex, path globs) and whether entries carry an id/name/description field
   that could serve as a **stable identity** for a third-party installer to own, update,
   and remove one entry idempotently.
4. **Event catalogue.** Every event name, verbatim, with when it fires. Group them:
   session lifecycle, prompt submit, pre/post tool use, file edit, command execution,
   notification, stop/finish, compaction, subagent, error.
5. **Invocation.** How is the hook executed? Shell command string? argv array? A JS/TS
   module the client imports? An HTTP endpoint? What is the working directory, the shell
   used, `$PATH` handling, and are there timeouts / concurrency / ordering guarantees?
6. **Input payload — verbatim.** How does the hook receive event data: JSON on **stdin**
   (give the exact schema/keys per event), **env vars** (exact names), **argv**, or
   template interpolation into the command string (e.g. `$TOOL_NAME`, `{{file}}`)? Include
   a real payload example from the docs if one exists.
7. **Output / response contract — verbatim.** How does the hook influence the agent?
   Exit-code semantics (what does 0 / 1 / 2 / other mean, exactly?). Is stdout parsed —
   as text injected into context, or as a **JSON response object**? Give the exact response
   schema and every field (deny/allow/ask, modified input, added context, user message,
   `continue: false`, `systemMessage`, …). Where does stderr go? Is the hook's output shown
   to the user, the model, both, or neither?
8. **Reliability & limits.** Timeout default and override. What happens on non-zero exit,
   on malformed output, on a missing binary? Are hooks run in parallel? Is the event
   blocking (agent waits) or fire-and-forget?
9. **Security posture.** Does the client warn/prompt before running hooks from a repo?
   Any trust/approval/allowlist mechanism, snapshotting of config at session start, or
   docs warning that hooks are arbitrary code execution? Quote the vendor's own wording.
10. **Third-party installability.** Realistically, can an external tool install a hook for
    this client by editing files — or is it UI-only / cloud-only / requires the vendor's
    own CLI? Any documented "config is snapshotted at startup" gotcha that means an
    installed hook needs a client restart?
11. **Trampoline viability.** Could a hook here be a *single generic command* (e.g.
    `grim hook run --client <name> --event <E>`) that receives the native payload and
    returns the native response? Name the blockers you see (e.g. handler must be a JS
    module, no stdin, response must be a typed object, hook must be an in-process
    function).

## Deliverable

1. Write the **full** findings — long, quote-heavy, with a `## Sources` table
   (`| URL | what it establishes | fetched |`) — to:
   `/tmp/claude-1000/-mnt-wsl-share-dev-grimoire-grimoire/dd45c0cc-03cc-49e4-a3f0-c74b48f12672/scratchpad/hooks-research/<vendor>.md`
2. Return as your final message a **condensed report, ≤700 words**, same section order,
   leading with a one-line verdict of the form:
   `VERDICT: <none | native-shell-hooks | js-plugin-hooks | notify-only | ui-only | unclear>`
   followed by a **confidence** line (high / medium / low) and what would raise it.

Tools: you have web access. If `WebSearch` / `WebFetch` are not loaded, run
`ToolSearch("select:WebSearch,WebFetch")` first. Do not modify anything in the grimoire
repository; your only write is the file above.
