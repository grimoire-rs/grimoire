# Research: Security & Trust Surface of Forge-Backed Ratings

<!--
Owner: hex-architect run 2026-08-17 (security/compliance research axis)
Handoff to: architect (ADR), /hex-plan, /hex-review (security perspective)
Extends: research_rating_backends.md — read that first; this file assumes its
settled design (forge-as-database, opaque handles, derived-not-stored marker
mapping, static read / live write split) and does not restate it.
-->

## Metadata

**Date:** 2026-08-17
**Domain:** security
**Triggered by:** [#82](https://github.com/grimoire-rs/grimoire/issues/82) — Ratings for catalog artifacts
**Expires:** 2027-02-17 (re-verify forge permission models and OAuth guidance)

## Direct Answer

The settled design is broadly sound and reuses good precedent (stdin credential
hand-off, `secrecy`, absent-is-first-class). Two findings change what ships:

1. **A GitHub App cannot implement this feature at all.** GitHub Apps have no
   Discussions permission — confirmed against GitHub's own permissions
   reference (§6). Any plan that assumes a GitHub App is the bot identity is
   wrong; the correct shape is a fine-grained PAT scoped to `discussions:
   read/write` on one repo, or `GITHUB_TOKEN` inside an Actions workflow.
2. **The marker-trust model as described is a Block-tier gap.** "The bot
   writes a marker, the tally job parses it back out" does not say the tally
   job restricts *which* content it trusts. Every bot that does this
   pattern at scale (§4) filters by comment/post **authorship**, not by
   marker text alone — anyone who can comment can otherwise forge or
   duplicate a `grim-ref` marker and redirect votes onto a different
   package. This must become an explicit invariant before any write path is
   built, not an implementation detail left to the builder.

Everything else (stdin over argv/env, `secrecy`/`Zeroizing`, token scope
breadth, abuse/privacy) is Warn/Suggest-tier — real, worth one sentence each
in the design doc, none blocking.

## Threat Table

| # | Threat | Likelihood | Impact | Mitigation | In settled design? |
|---|---|---|---|---|---|
| T1 | Marker forgery/duplication by any commenter redirects votes to a different `ref` | **High** if unmitigated — trivial, one public comment | **High** — silently corrupts `ratings.json`, cheaper than sockpuppet farming (§5) | Tally job trusts a `grim-ref` marker **only** when it appears in content authored by the bot's own account/App identity, and only in the top-level post (not replies) | **No — must be added.** Block |
| T2 | GitHub App assumed as bot identity | Certain if attempted | Feature cannot ship — GitHub Apps have no Discussions permission | Use a fine-grained PAT (`discussions: read/write`, one repo) or `GITHUB_TOKEN` in Actions | **No — corrects an assumption.** Block if planned, otherwise moot |
| T3 | VS Code session token carries full account/repo scope, not just "vote" | Certain (by construction of `getSession`) | Medium — any bug in grim's own handling leaks a broad-scope token, not a narrow one | Request the narrowest scope array at the call site (`public_repo` is the achievable floor — classic OAuth has no finer Discussions scope); never let the token leave process memory except the one outbound TLS call; document the breadth to users | Partially — token-scope minimization not yet stated | Warn |
| T4 | Token read into a bare `String` before wrapping, leaks via panic/log/core dump | Low if `login.rs` pattern is copied, Medium if a builder free-hands it | High if it happens (credential in cleartext in memory/log) | Reuse `login.rs:156-176` verbatim: read into `Zeroizing<String>`, wrap in `SecretString` immediately, `expose_secret()` only at the HTTP call site, never in a `Debug`/`Serialize` struct | Precedent exists in-repo, not yet pointed at from the rating design | Suggest (already solved — just copy it) |
| T5 | argv/env var token exposure (`ps`, `/proc/<pid>/environ`, CI log echo) | N/A — design already avoids this | N/A | stdin hand-off, as designed | **Yes.** No action |
| T6 | ref content breaks the HTML comment or surrounding markdown | Low — `ref` is OCI-charset-constrained by the existing `Identifier`/`ArtifactRef` validation, no `<`, `>`, `--`, backticks possible | Low if it did happen (rendering glitch, not code execution) | Verify at implementation time that the marker embeds the same validated identifier type used elsewhere on the write path, never a raw string reconstructed from untrusted input | Yes, by construction — needs one implementation-time check, not new code | Suggest |
| T7 | Rating aggregate used to amplify a typosquat/malicious package (fake popularity) | Depends entirely on T1 — high if T1 unmitigated, low once fixed | High — this is the actual "why does a rating number matter" security case (§5) | Fix T1; do not sign the aggregate (over-engineering — the index itself is unsigned, see quality-security.md) | Inherits T1's status | Warn (tracks T1) |
| T8 | GitHub has no native self-upvote block (unlike GitLab, which refuses award emoji on your own issue) | Medium — a malicious author can self-boost on GitHub specifically | Low–Medium — one platform's asymmetry, not exploitable at scale without also beating rate limits | Document the asymmetry; do not build custom self-vote detection for v1 (YAGNI, matches "cheapest reversible mechanism") | No — undocumented gap | Warn |
| T9 | Sockpuppet/brigading vote manipulation | Medium, bounded by forge rate limits (80 content-creates/min on GitHub) | Medium | Inherited from the forge (rate limits, abuse reporting); grim builds no competing anti-abuse system | Yes, by design (forge-as-moderator pattern) | Suggest |
| T10 | Vote reveals which account rated which package | Certain, but the action is already public on the forge by construction | Low — no new disclosure; `ratings.json` stores counts only, never voter identity | One line of UX copy: "voting posts publicly to your GitHub/GitLab account" | Yes in data shape (count-only); no UX copy yet | Suggest |
| T11 | `strace`/`ptrace` by a co-resident same-uid process reads the stdin pipe | Low — requires a hostile process already running as the same OS user | High if it happened, but this is true of *any* local-credential-handling scheme, not specific to this design | None available industry-wide; out of scope | N/A — inherent to local process credential hand-off | Suggest (name it, don't chase it) |

## 1. Token hand-off across the process boundary

**argv** lands in `/proc/<pid>/cmdline` on Linux, which is world-readable —
any local user runs `ps -ef` and reads it
([smallstep](https://smallstep.com/blog/command-line-secrets/),
[codestudy](https://www.codestudy.net/blog/hiding-secret-from-command-line-parameter-on-unix/)).
Windows has no `/proc`, but command-line strings are visible through
`Win32_Process.CommandLine` (WMI) and Task Manager's "Command line" column to
any process with `PROCESS_QUERY_INFORMATION` on same-user processes — argv is
not meaningfully protected on any of the three platforms.

**Environment variables** land in `/proc/<pid>/environ`, which — unlike
`cmdline` — is **not** world-readable on Linux (owner + root only); still
worse than stdin for two reasons: every grandchild process the child spawns
inherits the environment unless explicitly stripped, and CI runners routinely
echo environment into build logs by accident (`env`, `printenv`, `set -x`,
debug flags) — a well-documented leakage class distinct from the local-read
risk. macOS is stricter than Linux here (reading another process's environ
generally requires root even same-user, due to platform hardening); Windows'
posture is closer to Linux — same-user tools (Process Hacker-class) can read
it, root/Administrator always can.

**stdin** appears in neither `/proc/<pid>/cmdline` nor `/environ`, is not
inherited by grandchildren (each child's fd 0 is whatever the immediate
parent wired, not globally visible), and is not exposed by `ps`. This is why
current guidance converges on stdin/file-descriptor hand-off over argv or env
for CLI secrets
([smallstep](https://smallstep.com/blog/command-line-secrets/); reinforced by
[execa's input docs](https://github.com/sindresorhus/execa/blob/HEAD/docs/input.md)
recommending the same for Node child-process secret hand-off, which is
exactly the VS Code → `grim` shape here). On Windows, an anonymous pipe
created via `CreateProcess`'s `STARTF_USESTDHANDLES` is private to the
parent/child handle table and not enumerable by an unrelated process without
a handle-duplication exploit requiring elevated privilege — at least as safe
as the Linux case, arguably safer since there is no ptrace-equivalent
available to an unprivileged same-user process on modern Windows.

**Residual risks of stdin itself**, none of them design flaws — they are the
floor for any local process handing a secret to another local process:

- A same-uid hostile process running `strace -p <pid>` (Linux) or the
  platform equivalent can read the pipe as it is written. This is true for
  every local IPC mechanism (stdin, a Unix socket, a named pipe) — it is not
  a reason to prefer one over another, and no mitigation exists short of not
  running attacker code as your own user.
- Shell history is **not** a risk for the VS Code → `grim` call specifically,
  because the hand-off is programmatic (Node `child_process.spawn` writing to
  `child.stdin`), never a human typing `echo $TOKEN | grim rate …` at an
  interactive shell. It *is* a residual risk for the existing
  `--password-stdin` precedent when a human invokes it manually (`echo
  $TOKEN | grim login` leaks via the `echo`, not the pipe) — worth a doc note
  if `--token-stdin` is ever documented for manual use, not a code fix.
- Core dumps: a coredump captures whatever is in process memory at crash
  time. `login.rs` already avoids widening this window by reading directly
  into `Zeroizing<String>` (`src/command/login.rs:156-176`, read at :161,
  wrapped at :176) rather than a bare `String` — the rating command must copy
  this pattern exactly (see §3/T4).

**Verdict**: no action needed on the hand-off mechanism itself. T5 in the
threat table.

## 2. Token scope and audience

[RFC 9700](https://www.rfc-editor.org/info/rfc9700/) (OAuth 2.0 Security BCP)
requires the **resource server** (GitHub/GitLab) to validate the token's
audience and refuse mis-directed requests — that obligation is GitHub/GitLab's,
already met by their own token issuance, and not something grim implements or
needs to. [RFC 8252](https://www.rfc-editor.org/rfc/rfc8252.html) (OAuth for
Native Apps) is aimed at authorization-code interception between
co-installed native apps (PKCE is the relevant mitigation there); it does not
directly bear on an already-issued bearer token being reused by a second
local process the *same* application user explicitly invoked.

The real issue is **scope breadth, not audience confusion**. VS Code's
`authentication.getSession(providerId, scopes, options)` accepts a
caller-specified `scopes` array per call
([VS Code API reference](https://code.visualstudio.com/api/references/vscode-api#authentication)),
but GitHub's built-in `github` provider is one registered OAuth App shared by
every extension that calls it — in practice a session satisfying a narrower
scope request is returned as-is if a broader-scoped session already exists
for that provider (common VS Code extension behavior; requesting fewer scopes
does not shrink an already-issued broader token). The pragmatic floor,
per classic OAuth scope granularity, is **`public_repo`** — GitHub's classic
scope model has no Discussions-specific scope, so `public_repo` (or whatever
the extension's other features already hold, likely `repo`) is the narrowest
realistically obtainable token via the built-in provider.

[RFC 8693](https://www.rfc-editor.org/info/rfc8693/) (Token Exchange) exists
precisely to downscope a broad token to a narrow one via a security token
service — but standing one up reintroduces the "operate a service" cost the
whole rating design explicitly avoids (`research_rating_backends.md`'s Direct
Answer: "No service is operated, no database, no OAuth application"). **Not
recommended** — correctly absent from the settled design.

**Pragmatic minimum**: request the narrowest scope array explicitly at the
call site (do not silently inherit whatever scope another feature already
holds), and hold the code that touches the resulting token to a stricter bar
than ordinary data — no logging, no error-message interpolation, no retry
path that echoes it — because the blast radius of a leak is "your whole
GitHub session," not "one vote." One sentence of doc disclosure is
appropriate; a new OAuth app registration is not. T3 in the table.

## 3. Secret handling in Rust

The crate is already a dependency (`secrecy = "0.10"`, `Cargo.toml:67`) and
the exact pattern to copy already ships in `src/command/login.rs`:

```rust
// src/command/login.rs:156-176 — the pattern to reuse verbatim
let mut buf = Zeroizing::new(String::new());
std::io::stdin().read_to_string(&mut buf)?;
// … trim trailing newline …
Ok(SecretString::from(pass.to_string()))
```

Concrete pitfalls this avoids: reading into a bare `String` first (leaves an
un-zeroized heap allocation the allocator may reuse or that a core dump
captures in cleartext); deriving `Debug`/`Serialize` on any struct that holds
the token as a plain field (leaks into a `{:?}` log line or, worse, into a
`src/api/` report struct — the "always-present-null" convention there means
a naively-added token field would serialize into JSON output);
`format!("token {tok} …")` in an error/context string. `secrecy`'s
`SecretBox<T>` deliberately has no `Serialize` impl for this last reason, and
its `Debug` impl redacts to `[[REDACTED]]` by default
([secrecy docs](https://docs.rs/secrecy/latest/secrecy/)); `zeroize` wipes on
drop ([zeroize docs](https://docs.rs/zeroize/latest/zeroize/)).

**Idiomatic rule for the rating command**: `expose_secret()` is called
exactly once, at the point the HTTP client builds the `Authorization` header
for the GraphQL/REST call — never assigned to an intermediate `let` outside
that scope, never placed in a struct passed to `Printable`. T4 in the table
— Suggest-tier only because the pattern already exists in-repo; the risk is
a builder skipping it, not the pattern being unavailable.

## 4. Marker injection and content trust

This is the section worth being adversarial about, because the design as
handed off states a mechanism ("bot writes marker, tally job parses it back
out") without stating a trust boundary.

**Threat A — forgery/duplication by any commenter.** GitHub's "announcement"
Discussion category restricts who can post *top-level* posts to maintainers
— per the settled backend research, this is "the mechanism that makes
threads bot-owned." It does **not** restrict who can post *replies* under an
existing thread. Any user with read access to a public repo can reply, and
nothing stops a reply containing `<!-- grim-ref: some-other-popular-ref -->`.
If the tally job's marker search scans reply/comment bodies (or the whole
discussion thread, not just the bot-authored top-level post), an attacker can:

1. Post a forged marker in a reply under a **popular** package's thread,
   pointing at their own **malicious/typosquat** package's ref — redirecting
   that thread's organic upvotes onto the attacker's `ratings.json` entry.
2. Or post the same marker text in a brand-new (non-announcement, or a
   category the design didn't lock down) thread, creating a duplicate
   ref→thread mapping the tally job must arbitrarily disambiguate.

This is strictly cheaper than sockpuppet farming (§5, §7) — one comment,
zero new accounts, and it directly produces the "fake popularity" attack
class documented in package ecosystems generally (§5).

**The defense every bot that does this at scale actually uses is
authorship-scoped matching, not marker-content matching.** Confirmed from
real GitHub Actions bots that solve the identical "find our own marker
again" problem (sticky PR comments, `claude-code-action`'s sticky-comment
feature): "Search only comments authored by the expected automation
identity, because a contributor can place the marker in their own comment.
… This is critical for security since anyone could potentially include the
hidden marker in their own comments"
([anthropics/claude-code-action#411](https://github.com/anthropics/claude-code-action/pull/411),
[anthropics/claude-code-action#960](https://github.com/anthropics/claude-code-action/issues/960)).
The best-practice combination is author identity (bot account ID, not just a
matchable display name — display names can be renamed/impersonated) **plus**
the hidden marker, never the marker alone.

**Required invariant, not yet stated in the design**: the tally job trusts a
`grim-ref` marker only when (a) it appears in the **top-level post body**
(never a reply/comment), and (b) that post's author is the bot's own pinned
account ID or App identity. Both conditions, not either alone.

**Threat B — ref content breaking the marker or surrounding markdown.**
Checked against the actual type: `ArtifactRef` (`src/oci/reference.rs:13`)
wraps an OCI `Identifier`, and grim already enforces OCI tag/name charset at
publish time (`validate_channel_value_rejects_illegal_oci_tag_charset`,
`src/command/publish.rs`). OCI reference charset excludes `<`, `>`, `--`, and
backticks by construction, so a `ref` sourced from that type cannot break out
of an HTML comment or the surrounding markdown. This closes the classic
"content escapes its delimiter" injection vector **by construction** —
contingent on the write path actually embedding the same validated type end
to end, rather than reconstructing a string from less-trusted input. Worth
one implementation-time check, not new validation code. T6 in the table.

**Threat C — parser-differential risk.** A naive, unanchored regex parsing
`<!-- grim-ref: (.*) -->` is a separate bug class from forgery (what GitHub's
markdown renderer displays vs. what grim's regex extracts could diverge on
crafted or duplicated comment markup). Mitigation: anchor the regex, take the
first match only, and — most importantly — this class collapses to
irrelevant once Threat A's authorship filter is in place, since an
attacker-controlled comment is never parsed as a marker source regardless of
how it's formatted.

**Grade: Block.** This must be an explicit, named invariant in the plan
before any write/tally code lands — "parse markers from bot-authored
top-level posts only" — not an implementation detail a builder might or
might not get right.

## 5. Supply-chain / trust surface of the data

Once a rating exists, it is a target, not just a UX nicety. VS Code
Marketplace precedent: malicious extensions do inflate downloads/stars to
appear trustworthy, and "it is easy to publish an extension and trick rating
and downloads"
([ReversingLabs](https://www.reversinglabs.com/blog/malicious-vs-code-fake-image),
[The Red Guild](https://blog.theredguild.org/detecting-malicious-vscode-extensions-an-exploration/)).
General package-ecosystem precedent: typosquatting campaigns have
"artificially inflat[ed] packages' apparent credibility through GitHub stars
manipulation and fabricated download counts"
([TheHackerNews, 2024 npm campaign](https://thehackernews.com/2024/12/thousands-download-malicious-npm.html)).

For Grimoire specifically, the cheapest version of this attack is not
sockpuppet farming — it's Threat A (§4): forging a marker to redirect an
**existing, organic** vote count from a popular package onto a malicious
one. **A rating number is a meaningful new trust boundary conditional on
Threat A**: with the authorship fix in place, the only way to inflate a
count is genuine votes from real forge accounts, which is squarely the
forge's own abuse-prevention problem (§7), not grim's. Without the fix, it's
grim's bug, wearing the forge's rate limits as a false sense of security.

**Should the aggregate be signed?** No — over-engineering given the current
state of the rest of the system. The index itself carries no signature
scheme at all (`quality-security.md`: "No signature verification exists. …
Do not audit for it and never claim it"). An attacker who can already tamper
the unsigned index to redirect an *install* (arbitrary code execution, the
actually catastrophic attack) has no reason to separately bother forging a
*rating* (a display number) — signing the smaller, less valuable artifact
while the larger one stays unsigned is security theater. Revisit only if/when
the index itself gains an integrity story.

## 6. Bot identity and least privilege

**GitHub.** Verified against GitHub's own documentation directly (not
inferred): the GitHub App permissions list has **no Discussions
permission** at all
([choosing permissions for a GitHub App](https://docs.github.com/en/apps/creating-github-apps/registering-a-github-app/choosing-permissions-for-a-github-app)
— exhaustively checked, the word "discussion" does not appear). A GitHub App
literally cannot create discussions or toggle upvotes today — this rules out
"use a GitHub App" as an option, full stop, and is worth stating plainly
because it is the kind of assumption an architect reaches for by default
when "least privilege" comes up.

Fine-grained PATs **do** carry a `discussions` repository permission with
`read`/`write` levels
([managing your personal access tokens](https://docs.github.com/en/authentication/keeping-your-account-and-data-secure/managing-your-personal-access-tokens)
— confirmed in the full permission-name list). This is the correct
minimum-privilege shape for the tally/thread-creation bot: a fine-grained
PAT scoped to exactly one repo (the index/catalog repo), `discussions:
write` + the mandatory baseline `metadata: read`, nothing else — narrower
than the announce path's existing `GH_TOKEN`/`GITHUB_TOKEN` classic-scope
fallback, and it should **not** reuse that ladder's credential even though
it can reuse the same host-matched env-var mechanism (`grim` already has the
pattern; this needs its own, narrower credential).

`GITHUB_TOKEN` inside an Actions `permissions:` block accepts `discussions:
write` (same underlying permission namespace GitHub introduced alongside
fine-grained PATs). Good for a **scheduled tally/thread-bootstrap job**
running in CI — repo-scoped, expires at job end. **Not applicable** to the
live per-click vote from the VS Code extension, which legitimately needs the
end user's own token per the settled design; `GITHUB_TOKEN` is a CI-only
concept.

Escalation risk if the fine-grained PAT leaks: attacker can post/manage
discussions and toggle upvotes in **one repo only** — cannot read code,
cannot push, cannot touch secrets. Small, acceptable blast radius; exactly
what least privilege buys, and a sharp contrast with a leaked classic
`repo`-scope PAT.

**GitLab.** `CI_JOB_TOKEN` inherits the triggering user's actual role rather
than a fixed minimal set — GitLab's own design docs call this out as
violating least privilege
([Low-Privilege CI Job Tokens design doc](https://handbook.gitlab.com/handbook/engineering/architecture/design-documents/ci_job_token/)).
Acceptable for the read-only tally (award emoji reads are available even
unauthenticated on public projects, per the settled backend research), a
poor fit for a bot identity that should have a stable, auditable, minimal
permission set. **Project access tokens** (GitLab-created bot user,
project-scoped) are the correct shape — the same story as GitHub's
fine-grained PAT. Award Emoji is gated by the `api` scope; no
finer-grained award-emoji-only scope exists in the current project-access-token
model (mirrors GitHub's classic-scope gap) — the achievable floor is
"project-scoped `api` token," bounded to one project. GitLab's granular PAT
work is in flight
([Granular permissions for PATs — GA tracking item](https://gitlab.com/groups/gitlab-org/-/work_items/18554))
and worth a forward-looking note, not yet the documented default.

**Grade**: Block if a GitHub App was assumed anywhere in the plan (T2);
Suggest to use the narrower fine-grained-PAT/project-access-token shape
specifically for this bot identity, distinct from the broader announce-token
ladder.

## 7. Abuse and moderation

**Already mitigated by the forge**: GitLab natively refuses award emoji on
your own issue/work item (confirmed in the settled backend research — "GitLab
refuses award emoji on your own issue"). GitHub's secondary rate limit (80
content-creating requests/minute, 500/hour, already documented in the
settled research) throttles fast automated sockpuppet farming. Abuse
reporting exists as a human-moderation fallback on both forges.

**A real, undocumented platform asymmetry**: no source found (nor claimed in
the settled backend research) that GitHub blocks a discussion author from
upvoting their own thread — unlike GitLab's explicit, documented self-award
refusal. A malicious package author can self-boost their own rating on
GitHub specifically, with no native block. This should be **documented**,
not silently assumed away — T8 in the table.

**What remains grim's problem**: the marker-authorship fix (§4, T1) squarely
— that is a bug in grim's own tally logic, not the forge's abuse system.
Beyond that, building custom sockpuppet/brigading detection (account-age
gates, contribution-history discounting) is new complexity this design
doesn't need for v1 — consistent with the backend research's "cheapest
reversible mechanism" recommendation and Principle 4 (KISS). Name it as a
deferred, documented mitigation to reach for only if abuse is observed
post-launch, not something to build speculatively.

**Minimum that must exist before a public write path opens**: (1) the
authorship-scoped marker fix (§4, Block, must-have), (2) one line of
documentation about GitHub's self-vote gap (Warn, doc-only), (3) nothing
else new — everything else rides on native forge rate limits and abuse
reporting, which is the "forge as database, forge as moderator" pattern the
settled design already commits to.

## 8. Privacy

Kept proportionate, per the brief — the real obligations, not a compliance
treatise.

A forge upvote is tied to the voter's account identity — a pseudonymous but
reversible identifier (resolvable to a real person via their public profile),
which makes it personal data under a GDPR-style pseudonymization reading
(general principle, not GitHub/GitLab-specific: pseudonymous data "can no
longer be attributed to a specific data subject without additional
information" but remains personal data because that additional information
still exists and is held by the platform). **grim is not the data
controller for the vote event itself** — the vote lives and is processed on
GitHub/GitLab, under the privacy policy the user already agreed to by having
a forge account. What grim controls is narrower: `ratings.json` stores a
**count only**, never a list of voter identities — already true by the
settled design's "derived-not-stored mapping" (ref→count is recomputed from
the forge each run; nothing about individual voters is copied into any
grim-controlled artifact). No new disclosure is created — the vote is
already public on the forge by construction, same as a star or a reaction.

**Real obligation**: one line of UX copy before the vote fires — "voting
posts publicly to your GitHub/GitLab account" — so the action's visibility
is not a surprise. No data-retention policy is needed on grim's side because
grim retains nothing but a count. T10 in the table, Suggest-tier.

## Recommendation

Ship the design from `research_rating_backends.md` with one addition and one
correction, both cheap:

1. **Add** an explicit authorship-scoped marker-matching invariant to the
   plan before any write/tally code is written (§4, Block) — parse
   `grim-ref` only from the bot's own top-level post, never from replies or
   other authors' content. This is the one finding that changes the shape of
   the tally job, not just a review comment.
2. **Correct** any assumption that a GitHub App can serve as the bot
   identity (§6, Block-if-planned) — use a fine-grained PAT scoped to
   `discussions: read/write` on the index repo, or `GITHUB_TOKEN` inside an
   Actions workflow for the scheduled tally job; on GitLab, a project access
   token, not `CI_JOB_TOKEN`, for the same reason.

Everything else — stdin hand-off, `secrecy`/`Zeroizing`, ref charset safety,
token scope breadth, self-vote asymmetry, privacy — is a one-sentence doc
note or a "verify the existing pattern was actually copied" check, not new
design work. No signed aggregate, no separate OAuth app, no custom
anti-abuse system: all three would be disproportionate to the rest of the
(deliberately unsigned, deliberately service-free) system.

## Sources

| Source | Type | Relevance |
|---|---|---|
| [smallstep — command-line secrets](https://smallstep.com/blog/command-line-secrets/) | Blog | argv/`ps` exposure, stdin preferred |
| [codestudy — hiding secrets from cmdline params](https://www.codestudy.net/blog/hiding-secret-from-command-line-parameter-on-unix/) | Blog | `/proc/<pid>/cmdline` world-readability |
| [execa input docs](https://github.com/sindresorhus/execa/blob/HEAD/docs/input.md) | Docs | Node child-process stdin secret hand-off, same shape as VS Code→grim |
| [RFC 9700 — OAuth 2.0 Security BCP](https://www.rfc-editor.org/info/rfc9700/) | RFC | Audience restriction is the resource server's job, not the client's |
| [RFC 8252 — OAuth for Native Apps](https://www.rfc-editor.org/rfc/rfc8252.html) | RFC | Native-app token interception model (not directly on point here) |
| [RFC 8693 — OAuth Token Exchange](https://www.rfc-editor.org/info/rfc8693/) | RFC | Downscoping mechanism; rejected as disproportionate (needs a service) |
| [VS Code API — authentication](https://code.visualstudio.com/api/references/vscode-api#authentication) | Docs | `getSession` scopes-array signature |
| [secrecy crate docs](https://docs.rs/secrecy/latest/secrecy/) | Docs | `SecretBox` no-`Serialize`, redacted `Debug` |
| [zeroize crate docs](https://docs.rs/zeroize/latest/zeroize/) | Docs | Zero-on-drop guarantee |
| [claude-code-action #411](https://github.com/anthropics/claude-code-action/pull/411) | PR | Authorship-scoped sticky-comment matching (bot ID/name options) |
| [claude-code-action #960](https://github.com/anthropics/claude-code-action/issues/960) | Issue | "Anyone could include the hidden marker" — the exact T1 threat, named |
| [ReversingLabs — malicious VS Code extensions](https://www.reversinglabs.com/blog/malicious-vs-code-fake-image) | Blog | Marketplace rating/popularity gaming precedent |
| [The Red Guild — detecting malicious VS Code extensions](https://blog.theredguild.org/detecting-malicious-vscode-extensions-an-exploration/) | Blog | "Easy to trick rating and downloads" |
| [TheHackerNews — npm typosquat campaign](https://thehackernews.com/2024/12/thousands-download-malicious-npm.html) | News | Stars/download-count manipulation in a real package ecosystem |
| [GitHub — choosing permissions for a GitHub App](https://docs.github.com/en/apps/creating-github-apps/registering-a-github-app/choosing-permissions-for-a-github-app) | Docs | Confirms no Discussions permission exists for GitHub Apps |
| [GitHub — managing your personal access tokens](https://docs.github.com/en/authentication/keeping-your-account-and-data-secure/managing-your-personal-access-tokens) | Docs | Confirms fine-grained PAT `discussions: read/write` permission exists |
| [GitLab — Low-Privilege CI Job Tokens design doc](https://handbook.gitlab.com/handbook/engineering/architecture/design-documents/ci_job_token/) | Docs | `CI_JOB_TOKEN` least-privilege gap, GitLab's own admission |
| [GitLab — granular PAT permissions GA tracking item](https://gitlab.com/groups/gitlab-org/-/work_items/18554) | Tracker | Forward-looking note on GitLab's own fine-grained PAT effort |
| `src/command/login.rs:156-176` | Source | Existing `Zeroizing`/`SecretString` stdin pattern to reuse verbatim |
| `src/oci/reference.rs:13`, `src/command/publish.rs` (`validate_channel_value_rejects_illegal_oci_tag_charset`) | Source | Confirms `ref`/`ArtifactRef` charset already excludes marker-breaking characters |
| `.claude/rules/quality-security.md` | Rule | "No signature verification exists" — grounds the signed-aggregate rejection |
| `research_rating_backends.md` | Research | Settled design this file extends; forge API facts, identity findings |
