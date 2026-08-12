# Research: `git ls-remote` probe security (package-index freshness check)

**Axis**: domain/security · **Run**: `/hex-plan high` · **Scope**: replacing the
unconditional `git clone --depth 1` in `fetch_git` (`src/catalog/index_source.rs:186-231`)
with a cheap `git ls-remote` tip probe against a semi-trusted, user-configured
URL (`[[registries]]` `index = "git+https://..."`).

All claims below are either (a) sourced from git's own man pages / official
docs, (b) sourced from a CVE/advisory with a URL, or (c) **empirically
verified in this sandbox** against the locally installed **git 2.54.0** — those
are marked `[VERIFIED]` with the reproduction command. Sandbox: WSL2 Linux.

---

## 1. Attack surface of `git ls-remote <url>` with a semi-trusted URL

### 1a. `ext::` transport — arbitrary command execution

`ext::<command>[ <arguments>...]` bridges the git smart transport to an
arbitrary local command (`git-remote-ext(1)`); data to/from the command's
stdin/stdout is treated as the pack-protocol stream. This is a textbook
command-execution primitive when the URL is attacker-influenced — the classic
real-world instance is CVE-2022-25912 (simple-git RCE via `ext::` when a
library forwarded a user-supplied remote to `git clone`/`ls-remote`)
([GHSA-9p95-fxvg-qgq2](https://github.com/advisories/GHSA-9p95-fxvg-qgq2),
[Snyk SNYK-JS-SIMPLEGIT-3112221](https://security.snyk.io/vuln/SNYK-JS-SIMPLEGIT-3112221)).

`ext` is classified by git itself as a "known-dangerous" protocol and
**defaults to `never`** (`man git-config` → `protocol.allow`, verified locally,
§2). `[VERIFIED]`:

```
$ git ls-remote 'ext::touch /tmp/PWNED_ENV_TEST'
fatal: transport 'ext' not allowed
```

But the default is **not immutable** — it is overridden by ambient config or
env (§1f, §2). `[VERIFIED]` with an ambient `GIT_ALLOW_PROTOCOL=ext` (e.g. set
by an unrelated tool earlier in the same shell/CI job) and *no* attacker
control of grim's own invocation:

```
$ GIT_ALLOW_PROTOCOL=ext git ls-remote 'ext::touch /tmp/PWNED_ENV_TEST'
$ test -f /tmp/PWNED_ENV_TEST && echo EXECUTED
EXECUTED
```

### 1b. `--upload-pack=` / `-u` argument injection when the URL is mistaken for a flag

`git ls-remote` takes `--upload-pack=<exec>` (`git-ls-remote(1)`) — the path of
the program run on the far end to talk pack protocol. If grim ever hands git a
raw string that begins with `-` without a `--` end-of-options guard first, git
parses it as this flag instead of a positional URL (CWE-88, the same class as
CVE-2017-1000117, [Bugzilla
1480386](https://bugzilla.redhat.com/show_bug.cgi?id=1480386);
background/writeups: [safeguard.sh](https://safeguard.sh/resources/blog/argument-injection-in-git-and-mercurial),
[Snyk](https://snyk.io/blog/argument-injection-when-using-git-and-mercurial/)).
`ls-remote` parses its args via `parse_options(..., PARSE_OPT_STOP_AT_NON_OPTION)`
(`builtin/ls-remote.c`), so it *does* stop at the first non-flag-shaped token —
but a malicious "URL" that is itself shaped like `--upload-pack=...` never
reaches that stop condition; it's consumed as the flag.

`[VERIFIED]` — two-argument PoC (a second, legitimate local repo positional
supplies the connection target the injected `--upload-pack` value then runs
against):

```
$ git ls-remote '--upload-pack=touch /tmp/PWNED_MARKER; echo x #' "$(pwd)/victim.git"
fatal: Could not read from remote repository. …
$ test -f /tmp/PWNED_MARKER && echo EXECUTED
EXECUTED
```

**Sharper, single-token variant specific to `ls-remote`'s semantics** (not
shared by `clone`): when `ls-remote` is invoked with **zero** repository
positionals left after option parsing, it does not error — it falls back to
the *currently checked-out repository's configured remote* (tracking branch,
else `origin`). If the git subprocess's `cwd` is left at grim's own ambient
process directory (unset `current_dir`, as `fetch_git` does today —
`src/catalog/index_source.rs:198-209`), and that directory happens to be
*any* git working tree with a remote configured — extremely likely for a
dev-workflow CLI usually invoked from inside a project checkout — then **the
single config value `--upload-pack=<payload>` alone is a complete RCE**, no
second attacker-controlled argument needed:

```
$ cd some-project-with-a-configured-origin-remote/
$ git ls-remote --upload-pack='touch /tmp/PWNED4_MARKER; echo pwned #'
fatal: protocol error: bad line length character: pwne
$ test -f /tmp/PWNED4_MARKER && echo EXECUTED
EXECUTED
```

**This means the planned `ls-remote` probe is strictly more dangerous than
today's `clone` code on this specific axis** unless it both (i) prefixes `--`
before the URL and (ii) pins `current_dir` to a location that is never the
caller's own working tree — see §6.

**The fix**: `--` end-of-options (§3) closes this completely. `[VERIFIED]`:

```
$ git ls-remote -- '--upload-pack=touch /tmp/PWNED5_MARKER; echo pwned #'
fatal: strange pathname '--upload-pack=touch /tmp/PWNED5_MARKER; echo pwned #' blocked
```

(Git's own `looks_like_command_line_option()` guard rejects a dash-leading
positional even *after* `--`, rather than silently treating it as a literal
URL — belt-and-suspenders on top of the `--` fix itself.)

### 1c. `file://` and local-path traversal

`protocol.file.allow` defaults to `user` (allowed unless
`GIT_PROTOCOL_FROM_USER=0`) — a `file://` or bare local-path URL in the index
locator is honored by default and will `ls-remote` an arbitrary local path
readable by the grim process (information disclosure of ref names / local
repo enumeration, not RCE by itself). Relevant if the index locator is ever
taken from a lower-trust source than "the user's own config file" (it isn't,
today — `[[registries]]` is user-authored — but flagging per the brief).

### 1d. SSH URL forms — agent / host-key interaction, not code exec by default

`ssh://host/path`, `git@host:path`, and `host:path` (scp-like) all route
through the `ssh` protocol, defaulting to `always`-allowed. A hostile
`index` value here does **not** get argument-injection into the *ssh* command
line the way pre-2.7.6/2.14.1 git did (CVE-2017-1000117, fixed by inserting
`--` before the transport's own hostname argument
internally — [Phabricator T12961](https://secure.phabricator.com/T12961),
[git 2.14.1+/2.7.6+ fix](https://bugzilla.redhat.com/show_bug.cgi?id=1480386)).
What it *does* do on an otherwise-unremarkable host: silently invoke the
configured SSH client (`GIT_SSH`/`GIT_SSH_COMMAND`/`core.sshCommand`, else
`ssh`), which will consult `~/.ssh/known_hosts` and may block on an
interactive host-key-verification prompt or an SSH agent passphrase prompt —
this is the hang surface, not an exec surface, covered under §4.
`GIT_TERMINAL_PROMPT=0` does **not** suppress this — that variable only gates
git's *own* terminal prompts (HTTP auth), not the ssh subprocess's own TTY
interaction (`man git` → `GIT_TERMINAL_PROMPT`, verified locally). SSH's own
`BatchMode=yes` (or `-o BatchMode=yes`) is the actual lever, and grim
currently sets neither `GIT_SSH_COMMAND` nor any ssh-side batch flag.

### 1e. HTTP redirect to a different host

Git's own HTTP client (`libcurl`-backed) follows redirects by default with no
count cap exposed as a *disable* switch beyond `http.followRedirects`
(`initial` / `false` / `true` — see §2). An `index` URL under attacker
influence could 30x to a different host; with the default (`initial`, meaning
"only the initial ref-discovery request may redirect, then the discovered
final URL is pinned for the rest of the exchange") this is git's own
documented mitigation for the "redirect after ref advertisement changes the
push/fetch target mid-transaction" class, not a general redirect-following
disablement. This is a materially different (safer) situation than the
project's own `reqwest`-based forge client, which the codebase's own comment
explains disables redirects entirely because *it* replays a bearer/PAT header
on 3xx (`src/catalog/forge.rs:266-274`) — `git ls-remote` carries no such
static header to leak (the credential-helper protocol is a separate,
per-request negotiation, not a static header — see §1f), so the leak class
`build_client`'s comment is guarding against does not directly transfer. The
config knob that *would* fully pin transport (`http.followRedirects=false`)
is still worth setting for a semi-trusted URL, since "initial-only" still
permits one host hop before pinning.

### 1f. Credential-helper invocation

`ls-remote` over `https://` with no embedded credentials will invoke
`credential.helper` (ambient — whatever's configured globally/system-wide) to
ask for a username/password if the server responds 401. This is exactly the
**Clone2Leak** (RyotaK / GMO Flatt Security, disclosed January 2025) attack
class: a malicious URL or server response can trick a credential helper into
handing credentials meant for one host to another.
[Clone2Leak writeup](https://flatt.tech/research/posts/clone2leak-your-git-credentials-belong-to-us/),
[BleepingComputer summary](https://www.bleepingcomputer.com/news/security/clone2leak-attacks-exploit-git-flaws-to-steal-credentials/):

- **CR/LF injection into credential-protocol fields** via a crafted URL
  (`http://%0dprotocol=https%0dhost=github.com%0d@localhost:PORT/`) exploited
  helpers whose parser treated `\r` as a field separator when git's own
  credential protocol only uses `\n`. Git's own fix:
  `credential.protectProtocol` (default `true` since the fix), which rejects
  credential values containing a bare CR. Relevant here only if grim's own
  ambient `credential.helper` predates the fix or is a third-party one with
  the same parsing bug — grim doesn't control that, but URL-scoping (below)
  bounds the blast radius regardless.
- **CVE-2024-53858** (GitHub CLI / Codespaces helper): a helper that answers
  for *any* host once an enterprise env var is present, independent of the
  URL git actually asked about. Not applicable to grim's own code (grim
  doesn't ship a credential helper), but a reason to URL-scope any helper
  grim *does* configure — exactly the pattern already used in
  `job_token_credential_config` (`src/catalog/index_announce.rs:94-129`,
  `credential.https://<host>.helper=…`, single-host-scoped).
- **Mitigation git ships**: `credential.helper=` (empty value, config-only,
  clears the list built so far) disables all ambient helpers for the
  invocation when passed as an additional `-c credential.helper=` **before**
  any real helper is wanted; grim's forge/announce paths intentionally do the
  opposite (append a scoped fallback helper) because they need one. For a
  bare `ls-remote` probe that needs no push/pull of content, the correct
  default is *no* explicit credential config at all: let ambient helpers work
  for private indexes exactly as the current `fetch_git` code already assumes
  ("a private index needs ambient git credentials", `index_source.rs:206`) —
  this is intentional today and should stay intentional; Clone2Leak's actual
  fixed versions (GitHub Desktop ≥ 3.4.12, Git Credential Manager ≥ 2.6.1, Git
  LFS ≥ 3.6.1, GitHub CLI ≥ 2.63.0) are the relevant patch targets, not
  something grim's invocation shape can itself remediate.

---

## 2. Config knobs that close each hole

Verified against `man git-config` (git 2.54.0, local) plus `man git`, and
empirically confirmed for the ones marked `[VERIFIED]`.

| Knob | Form | What it prevents | `-c` (pre-verb) vs env |
|---|---|---|---|
| `protocol.allow` | `-c protocol.allow=never` | Default-deny **every** protocol not given an explicit `protocol.<name>.allow` | Either; `-c` must precede the subcommand |
| `protocol.<name>.allow` | `-c protocol.ext.allow=never` | The named transport (`ext`, `file`, `git`, `ssh`, `http` — note **not** `https`, configure separately) specifically. `ext` already defaults to `never`; the explicit `-c` is defense-in-depth against an ambient config that weakened it | `-c`, must precede subcommand. `[VERIFIED]`: an ambient `-c protocol.ext.allow=always` set *before* grim's own `-c protocol.ext.allow=never` is correctly overridden (last `-c` wins) |
| `GIT_ALLOW_PROTOCOL` | env, colon-separated allowlist | **Everything `protocol.*.allow` prevents, and more** — per `git-config(1)`: setting it "behaves as if `protocol.allow` is set to `never`, and each of the listed protocols has `protocol.<name>.allow` set to `always`", **overriding any existing configuration** | Env only. **`[VERIFIED]` critical finding**: an ambient `GIT_ALLOW_PROTOCOL=ext` env var (inherited from the parent shell/CI job — not attacker-controlled config, just an unrelated tool's leftover env) **overrides grim's own explicit `-c protocol.ext.allow=never`** — the `-c` flag alone is *not* sufficient. Only an explicit `GIT_ALLOW_PROTOCOL=<safe-list>` set on the child (which replaces, not merges with, whatever the parent process had) closes this — `[VERIFIED]` that an explicit safe value on the child wins over an inherited `ext`-permitting one |
| `http.followRedirects` | `-c http.followRedirects=false` | Any host hop at all, including the "initial ref-advertisement request only" default (`initial`) `git-config(1)` documents | `-c` |
| `core.askPass` / `GIT_ASKPASS` | env `GIT_ASKPASS=echo` (or point at a program that always fails fast) or `-c core.askPass=` | A GUI/graphical or scripted askpass helper answering a password prompt outside the terminal — **not** covered by `GIT_TERMINAL_PROMPT=0`, which only gates git's own terminal prompts | Either works; env is simpler for a one-shot subprocess |
| `SSH_ASKPASS` | `.env_remove("SSH_ASKPASS")` or pin a no-op | Same class, ssh-side | Env |
| `GIT_TERMINAL_PROMPT` | env `=0` | git's own terminal auth prompt (already set by the existing code) — does **not** cover ssh's own prompting (host-key TOFU, agent passphrase) | Env |
| `credential.helper=` | `-c credential.helper=` (empty) | Disables all ambient helpers for this one invocation. **Do not set this for the `ls-remote` probe** — the existing code intentionally leaves ambient helpers active so private indexes keep working (`index_source.rs:206`); only use this pattern where no credential is ever wanted |
| `safe.directory` | n/a for a *remote* URL | Governs whether git will operate on a **local** repository owned by a different UID (post-CVE-2022-24765 mitigation) — irrelevant to `ls-remote` against a remote URL, relevant only if `current_dir` is pinned to a shared/multi-tenant directory that git then treats as the repo context; pin `current_dir` to a private tempdir owned by the grim process to sidestep this entirely |
| `uploadpack.*` | server-side only | Not settable by the client (grim); server-side knobs (`uploadpack.hideRefs`, `uploadpack.allowTipSHA1InWant`) govern what a well-behaved server chooses to advertise — irrelevant to hardening the *client* invocation |

**`-c` vs env, precisely**: every `-c key=value` must appear **before** the
subcommand verb (`git -c protocol.ext.allow=never ls-remote …`, not after) —
already the established idiom in this codebase
(`src/catalog/index_announce.rs:626-627`, `with_credential`). Environment
variables (`GIT_ALLOW_PROTOCOL`, `GIT_TERMINAL_PROMPT`, `GIT_ASKPASS`,
`GIT_HTTP_LOW_SPEED_LIMIT`/`_TIME`) are set via `Command::env(...)` and are
**not** expressible as `-c` flags — they're read directly by git's C code
before/outside config parsing for exactly the cases (`GIT_ALLOW_PROTOCOL`)
where they're *designed* to have final say over config.

---

## 3. `--` end-of-options

- **Git version support**: `--` as a universal end-of-options marker is a
  property of `parse-options.c`, git's shared CLI argument parser, in use by
  essentially every porcelain and plumbing command for the entire modern git
  era (`parse-options.c` predates git 1.6). `ls-remote` calls
  `parse_options(argc, argv, prefix, options, ls_remote_usage,
  PARSE_OPT_STOP_AT_NON_OPTION)` (`builtin/ls-remote.c`, confirmed via the
  upstream source), which is the standard mechanism `--` terminates. There is
  no meaningful "minimum version" caveat to carry forward for this
  project's baseline (git ≥ 2.11.1/2.12, Jan–Mar 2017, is already the
  effective floor for `protocol.allow` itself, per §2, and `--` predates that
  by nearly a decade). `[VERIFIED]` on git 2.54.0.
- **What it prevents, exactly**: it is the fix for the entire "URL/ref/branch
  positional shaped like a flag" argument-injection class (CWE-88) — the same
  class as CVE-2017-1000117 and the `--upload-pack=` injection in §1b. It
  does **not** prevent the `ext::` transport (§1a, a *content* problem in an
  otherwise-valid positional, not a parsing problem) or SSH hostname
  injection inside a `ssh://` URL's *authority* component (that's a URL-parsing
  concern already handled by git's own URL parser, not an argv-shape
  concern) — `--` only forces one specific token to be treated as positional
  regardless of its leading character.
- **Already the established idiom here**: `index_announce.rs`'s clone/push
  calls already put `--` before the repo URL and before push targets/branch
  names (`"clone", "--depth", "1", "--quiet", "--", &request.repo_url, …`,
  `index_announce.rs:229-237`). The **current unconditional `fetch_git` clone
  in `index_source.rs` does not** (`.arg("clone").arg("--depth").arg("1")
  .arg("--quiet").arg(&url).arg(&tmp)`, no `--`, `index_source.rs:198-204`) —
  a pre-existing gap on the code this change touches, worth closing in the
  same diff regardless of whether the swarm keeps a clone fallback path.

---

## 4. Hang / DoS surface

**Git has no built-in overall/wall-clock command timeout.** Confirmed by the
absence of any `http.timeout`-style *connect* or *total* deadline in
`git-config(1)` — the only timeout-shaped knobs that exist are:

- `http.lowSpeedLimit` / `http.lowSpeedTime` (env override:
  `GIT_HTTP_LOW_SPEED_LIMIT` / `GIT_HTTP_LOW_SPEED_TIME`) — abort an **HTTP**
  transfer that has already started but is running below N bytes/sec for M
  seconds (`man git-config`, confirmed locally). This is the standard,
  git-native lever for the "slow server" case and should be set explicitly
  (git's compiled-in default is effectively "off" unless the user's own
  `~/.gitconfig` sets it).
- It does **not** bound: DNS resolution, initial TCP connect to an
  unreachable/black-holed host, the TLS handshake, or an SSH connection
  stalled before the pack-protocol stream starts (host-key prompt, agent
  wait, or a server that accepts the TCP connection but never speaks).
  git ships no `http.connectTimeout`-equivalent and no client-side SSH
  `ConnectTimeout` default — that is ssh's own `-o ConnectTimeout=N` /
  `~/.ssh/config`, which grim does not currently set via
  `GIT_SSH_COMMAND`.

**So `ls-remote` genuinely can block indefinitely** against an unreachable
host, a TCP black hole, or a server that completes the TCP/TLS handshake and
then never sends the pack-protocol greeting. This is unchanged from today's
`clone` call, which has the identical gap.

**Does the repo's existing `tokio::process` usage bound it? No — for either
the current or the planned code.** Both `git_output_impl`
(`index_announce.rs:609-640`) and `fetch_git`
(`index_source.rs:198-210`) call `.output().await` directly with no
`tokio::time::timeout(...)` wrapper anywhere in the call chain. This is a
pre-existing gap, not something the new `ls-remote` code inherits protection
from.

**The standard mitigation, and a Rust/tokio-specific pitfall to avoid when
adding it**: wrap the awaited call in `tokio::time::timeout(DURATION, …)`.
Critically, per tokio's own docs (`tokio::process::Command::kill_on_drop`),
**a `Child`/`.output()` future dropped by a losing `timeout()` race does
*not* kill the underlying process by default** — "by default, this value is
assumed to be `false`, meaning the next spawned process will not be killed
on drop, similar to the behavior of the standard library." Without
`.kill_on_drop(true)` on the `Command` builder, a timed-out `git ls-remote`
becomes an orphaned process that keeps running (and can keep holding a
TCP/SSH connection open) after grim itself has moved on and reported the
probe as failed/timed-out. The fix is `.kill_on_drop(true)` on the
`Command`, or an explicit `child.kill().await` in the timeout branch instead
of a bare `.output()` future.

`GIT_HTTP_LOW_SPEED_LIMIT`/`GIT_HTTP_LOW_SPEED_TIME` are worth setting
regardless (they're the "accepted lever" the brief asks about, and they cost
nothing), but they are **not sufficient alone** — they only help once an HTTP
transfer is already underway. The wall-clock `tokio::time::timeout` +
`kill_on_drop(true)` pair is what actually bounds the probe end-to-end across
every transport (HTTP, SSH, git://, local).

---

## 5. CVE history (2022–2025 window)

Directly relevant to running git against an attacker-influenced URL/transport:

| CVE / advisory | Component | Class | Fixed | Relevance here |
|---|---|---|---|---|
| [CVE-2017-1000117](https://bugzilla.redhat.com/show_bug.cgi?id=1480386) | git core (ssh transport) | Argument injection via `ssh://` hostname | git 2.7.6, 2.8.6, 2.9.5, 2.10.4, 2.11.3+ | Origin of the `--` end-of-options fix pattern this project already follows on the announce path (§3) — ancient, but the *pattern* is exactly what's needed for the new `ls-remote` call |
| [CVE-2022-25912](https://github.com/advisories/GHSA-9p95-fxvg-qgq2) (simple-git) | Library wrapper, not git itself | `ext::` RCE via unsanitized remote URL forwarded to git | simple-git ≥ 3.15.0 | Direct precedent for §1a — the exact failure mode (URL under app control reaches git's `ext::` transport unfiltered) |
| CVE-2022-24439 → superseding [CVE-2023-40267](https://security.snyk.io/vuln/SNYK-PYTHON-GITPYTHON-5840584) (GitPython) | Library wrapper | Incomplete `--` fix, insecure non-multi options in `clone`/`clone_from` | GitPython patched releases | Cautionary: a partial/incorrect argument-injection fix can be *reintroduced*; validate the `--` placement against the actual final argv, not just "we added `--` somewhere" |
| [CVE-2024-53858](https://ssojet.com/blog/github-vulnerability-exposes-credentials-to-malicious-remote-urls-and-repositories/) ("Clone2Leak" cluster, GHSA + Flatt Security writeup above) | git credential-helper ecosystem (GitHub CLI/Desktop, Git Credential Manager, Git LFS — not git core) | Credential exfiltration to an unintended host via helper logic flaws | GitHub Desktop ≥ 3.4.12, GCM ≥ 2.6.1, Git LFS ≥ 3.6.1, GitHub CLI ≥ 2.63.0 | §1f — motivates URL-scoping any helper config grim itself adds (already done correctly for announce, `index_announce.rs:106-124`); `ls-remote` adds no new helper config so nothing new to scope |
| [CVE-2024-32002](https://amalmurali.me/posts/git-rce/) | git core (submodule + case-insensitive filesystem) | Malicious submodule hook execution on `clone --recursive` | 2.45.1, 2.44.1, 2.43.4, 2.42.2, 2.41.1, 2.40.2, 2.39.4 | Not directly applicable — `ls-remote` never checks out a working tree or processes `.gitmodules`; relevant only to the (retained?) `clone` code path if the swarm keeps one |
| [CVE-2024-32004](https://www.sentinelone.com/vulnerability-database/cve-2025-48384/) | git core (`clone`) | Local repo crafted to execute arbitrary code on clone | Same 2.4x line as above | Same caveat — `clone`-only, not `ls-remote` |
| [CVE-2025-48384](https://dgl.cx/2025/07/git-clone-submodule-cve-2025-48384) | git core (submodule config CR handling) | Post-checkout hook RCE via CR in submodule path | 2.43.7, 2.44.4, 2.45.4, 2.46.4, 2.47.3, 2.48.2, 2.49.1, 2.50.1 | `clone`-only, not `ls-remote`; strengthens the case for *not* falling back to a full clone when a cheap tip check suffices |

**No CVE was found specifically against `git ls-remote` itself** in this
window — its narrower surface (no working-tree checkout, no submodule
processing, no hook execution) is precisely *why* it's the safer choice for a
"just check the tip" probe, provided the invocation itself is hardened per
§1–§4. The CVE history in this window skews toward `clone`'s
working-tree/submodule/hook machinery, which `ls-remote` structurally avoids
by never fetching objects or writing a working tree.

---

## 6. Recommendation

**Minimum git version note**: none of the mitigations below need anything
newer than git 2.12 (`protocol.allow`/`protocol.<name>.allow`,
2017) or `GIT_ALLOW_PROTOCOL` (2.7.6/2.14.1 era, also 2017) — both long
predate any git version this project should reasonably require. `--` support
is universal. No version gate is needed in code.

**Exact hardened `git ls-remote` invocation** (argv + env), matching the
project's own announce-side idiom (`-c` before the verb, `--` before the URL,
`GIT_TERMINAL_PROMPT=0` already established) and closing the gaps that idiom
*doesn't* currently cover:

```rust
use std::time::Duration;

/// Wall-clock cap for the tip probe — git has no built-in equivalent
/// (see research doc §4). Sized like the sibling HTTP index fetch's
/// own timeout (`index_source.rs::HTTP_TIMEOUT`).
const LS_REMOTE_TIMEOUT: Duration = Duration::from_secs(15);

let mut cmd = tokio::process::Command::new("git");
cmd.arg("-c")
    .arg("protocol.ext.allow=never")   // defense-in-depth vs a weakened ambient *config*
    .arg("-c")
    .arg("http.followRedirects=false") // pin the transport to the configured host, no hop
    .arg("ls-remote")
    .arg("--exit-code")
    .arg("--")                         // end-of-options: url can never be parsed as a flag
    .arg(&url)
    .arg("HEAD")                       // narrow the ref set fetched to what's actually needed
    // Never run inside the caller's own cwd: ls-remote falls back to the
    // ambient repo's configured remote when it gets zero positionals, and a
    // single injected `--upload-pack=` token is a full RCE if cwd happens to
    // be *any* git working tree with a remote (see research doc §1b). A
    // fresh, private tempdir sidesteps that fallback and safe.directory
    // ownership checks in one move.
    .current_dir(&scratch_dir)
    .env("GIT_TERMINAL_PROMPT", "0")
    // Authoritative allowlist: unlike the `-c` above, this closes the gap
    // where an *ambient* GIT_ALLOW_PROTOCOL=ext env var (inherited from the
    // parent shell/CI job, not attacker-controlled) would otherwise
    // override the `-c` flag entirely (verified — see research doc §2).
    .env("GIT_ALLOW_PROTOCOL", "http:https:ssh:git")
    .env("GIT_ASKPASS", "echo")        // never let a GUI/script askpass answer silently
    .env_remove("SSH_ASKPASS")
    .env("GIT_HTTP_LOW_SPEED_LIMIT", "1000")
    .env("GIT_HTTP_LOW_SPEED_TIME", "10")
    .kill_on_drop(true);               // tokio does NOT kill on drop by default (verified, §4)

let output = tokio::time::timeout(LS_REMOTE_TIMEOUT, cmd.output())
    .await
    .map_err(|_| /* timed-out variant */)?
    .map_err(/* io error variant */)?;
```

**Validation to apply to the configured index URL before git ever sees it**:
none of the mitigations above are a substitute for a cheap upfront shape
check on the `[[registries]]` `index` value, mirroring what the announce path
already does at the forge layer (`verify_fork_push_url`,
`index_announce.rs:1010-1033`) — reject (config-load-time, not probe-time)
any locator whose scheme, after stripping a `git+` prefix, is not one of
`https://`, `ssh://`, `git://`, or a bare `scp`-like `host:path`/`user@host:path`
form — i.e. reject anything starting with `ext::`, `file://`, or a bare
absolute/relative filesystem path outright at config-parse time. This is
belt-and-suspenders on top of the runtime `protocol.*.allow`/
`GIT_ALLOW_PROTOCOL` gates (an operator who *wants* a local-path index for a
monorepo/offline setup is a legitimate, if narrower, use case — if that's
supported, gate it behind an explicit opt-in rather than silent
scheme-sniffing).

**On keeping vs. dropping the `clone` fallback**: if the plan retains a full
clone as a fallback for hosts/configs where `ls-remote`'s tip alone isn't
enough, apply the identical `--`/`current_dir`/`kill_on_drop`/timeout/
`GIT_ALLOW_PROTOCOL` hardening there too — today's `fetch_git` clone
(`index_source.rs:198-210`) has *none* of it, and per §1b the clone path,
while structurally different, shares the same missing-`--` root cause.

---

## Top 3 findings (for the pointer message)

1. **`GIT_ALLOW_PROTOCOL` env var beats `-c protocol.ext.allow=never`,
   `[VERIFIED]` empirically.** The announce path's existing idiom
   (`index_announce.rs:626-627`) is real protection against a weakened
   ambient *config*, but an ambient `GIT_ALLOW_PROTOCOL=ext` env var
   (not attacker-controlled — just inherited from the parent shell/CI job)
   silently overrides it. Only an explicit `GIT_ALLOW_PROTOCOL=<safe-list>`
   set on the child closes this. This applies equally to the *existing*
   announce/clone code, not just the new probe.
2. **`ls-remote` with zero positionals falls back to the ambient repo's
   configured remote — a single-token `--upload-pack=` config value is a
   complete RCE if `current_dir` isn't pinned away from the caller's own
   working tree, `[VERIFIED]` empirically.** This is *worse* than the
   current `clone`-based code (which requires two attacker-controlled
   tokens) precisely because of `ls-remote`'s no-args fallback semantics —
   `--` **and** an explicit non-repo `current_dir` are both required, not
   just one.
3. **Git has no wall-clock timeout, and `tokio::process` doesn't add one for
   free.** `GIT_HTTP_LOW_SPEED_LIMIT`/`_TIME` only bound an HTTP transfer
   already in progress, not TCP connect/SSH handshake stalls. Wrap the call
   in `tokio::time::timeout` **and** set `.kill_on_drop(true)` — without the
   latter, tokio's own docs confirm a timed-out git subprocess keeps running
   orphaned rather than being killed.

**Recommendation**: proceed with the `ls-remote` probe — its CVE history and
structural surface (no working tree, no submodule/hook execution) are
narrower than `clone`'s — but only with the full hardened invocation in §6:
`-c protocol.ext.allow=never` (existing idiom, keep), `--` before the URL
(new, matches the announce path's own pattern but currently absent from the
code being replaced), an explicit `GIT_ALLOW_PROTOCOL` env allowlist (new,
closes a real gap the existing idiom doesn't), a pinned non-caller
`current_dir` (new, closes the single-token fallback RCE unique to
`ls-remote`), and a `tokio::time::timeout` + `kill_on_drop(true)` pair (new,
git has no native equivalent).
