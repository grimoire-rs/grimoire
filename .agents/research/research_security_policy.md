# Research: SECURITY.md and the Supply-Chain Trust Model

<!--
Owner: Researcher (W3-A research gate)
Handoff to: meta-plan_promotion_1_0.md W3-A (SECURITY.md + threat/trust model)
Purpose: Evidence base for a SECURITY.md another agent writes. Not the policy text.
-->

## Metadata

**Date:** 2026-07-26
**Domain:** security policy, supply-chain trust, OSS governance mechanics
**Triggered by:** meta-plan_promotion_1_0.md, W3-A deep-research gate
**Expires:** 2027-01-26 (GitHub UI/API behavior and peer policies drift; re-verify before citing publicly)
**Method:** Fetched and read 5 peer SECURITY.md files (not summarized from memory); grepped grimoire's `src/` for every signature/cosign/notation/sigstore/attestation term and every archive/permission/env-injection surface named in `quality-security.md`; checked the GitHub REST API for grimoire-rs/grimoire's security settings; fetched GitHub's own docs for file-discovery precedence.

**Orchestrator amendment (2026-07-26, authenticated `gh`):** the researcher ran
unauthenticated and could not read `security_and_analysis`. Re-checked with
`gh auth`, results in Q4. Its Q4 caveat is resolved, not carried forward.

---

## Direct Answer

grim has a real, correctly-implemented content-**integrity** story (SHA-256 digest pinning, two-layer path-containment, CWE-770 size caps) and **zero** content-**authenticity** story (no cosign/notation/sigstore anywhere). That gap is exactly what peers who occupy grim's trust position state plainly rather than hide — `uv`'s SECURITY.md is the cleanest model for how to say "this is not a vulnerability, it's a design boundary" without either overclaiming or alarming readers. Separately, `quality-security.md`'s "Grimoire-Specific Attack Surfaces" list has **drifted from the code** in several places — none of them describe anything that exists in `src/` today, and citing them in a public SECURITY.md would be a false claim in one direction (signature validation) and a false alarm in others.

---

## Q1 — What a supply-chain-position SECURITY.md contains in 2026

Five peers fetched and read directly: **ORAS**, **cosign**, **uv**, **cargo-dist/dist**, and **Helm** (Helm 3 pushes/pulls charts to/from OCI registries, making it a closer peer than a random pick).

| Project | File location | Disclosure channel | SLA? | Threat-model / scope section? | Explicit non-guarantee? |
|---|---|---|---|---|---|
| **ORAS** | Root `SECURITY.md` is a 1-line stub → `oras.land` docs site | GitHub Security Advisory + `cncf-oras-security@lists.cncf.io` | Yes — 14 working days (high severity) | No | No |
| **cosign** | Root `SECURITY.md`, the sigstore-wide process copied verbatim across org repos | `security@sigstore.dev` (+ PGP) | Yes — 24h acknowledgment | No out-of-scope list | No |
| **uv** | Root `SECURITY.md` (scope only) **defers** contact to org default `astral-sh/.github/SECURITY.md` | `security@astral.sh`; GitHub Security Advisory | No | **Yes** — the repo-level file *is* a scope statement | **Yes — the standout.** *"uv can execute arbitrary code… **These are not considered vulnerabilities in uv.** If you think uv's stance in these areas can be hardened, please file an issue for a new feature."* |
| **cargo-dist / dist** | Root `SECURITY.md`, company-authored | GitHub private reporting **or** `ashley@axo.dev` | No | No | No — but an explicit conduct norm: *"Please do not report security vulnerabilities publicly"*, *"we prefer over-communication"* |
| **Helm** | Root stub → shared `helm/community/SECURITY.md` | `cncf-helm-security@lists.cncf.io` + PGP (5 keyholders) | Yes — 3 business days | **Yes** — explicit "When Not To Send A Report" | No |

**Majority behaviour:** email as primary intake (4/5); **no supported-versions table (0/5)**; **no dedicated "Threat Model" heading (0/5)** — scope language is folded into prose.

**What only one does (do not copy):** Helm's and sigstore's committee governance — named rosters, PGP keyholder tables, CVSS screenshots, 1/7/21-day release choreography. That is CNCF multi-maintainer scale. The two peers closest to grim's scale — **uv** and **cargo-dist** — are also the two that skip an SLA entirely. Best-effort, honestly stated, beats a number nobody can hold.

**Borrow, concretely:** uv's non-guarantee sentence structure; cargo-dist's conduct norm; Helm's "when not to send" framing without the apparatus.

**Multi-repo note:** three patterns exist for shared policy — GitHub's org-default `.github` repo (zero drift by construction), a hand-linked shared doc repo (drifts), external docs-site delegation (breaks in-GitHub reading). See Q4 for which applies here.

---

## Q2 — The honest limitation, verified against source

**Confirmed: grim has no signature verification anywhere.** Grepping `src/` for `cosign|sigstore|notation|attestation|signature|sign(ed|ing)?` matched only false positives: `signed_duration_since` (chrono), "signal", Rust function "signature", a PNG file-signature byte comment in a test, and GitHub's not-ready API response signatures during fork provisioning. **Zero hits about cryptographic signing of artifacts.**

**What grim verifies is content integrity, not publisher authenticity:**
- `src/oci/digest.rs`, `src/oci/access.rs:84` ("Implementations verify the bytes hash to `digest`"), `access/cached_access.rs` — every fetched blob's SHA-256 is checked against the digest named in the manifest/lockfile. That proves *the bytes you got are the bytes that were locked*; it says nothing about who produced them.
- The lockfile pins by digest, so a re-pull can't silently swap content — but the *first* `grim add` trusts whatever the registry serves, exactly like `docker pull` without `cosign verify`.

**Two adjacent things, checked and ruled out:**
1. **`--git` provenance annotations** (`src/oci/git_provenance.rs`) — opt-in, stamps `org.opencontainers.image.{revision,created,source}` from the publisher's own working tree. Unsigned self-attested metadata; nothing verifies it on install.
2. **`grim login`'s verify ping** (`src/auth/verify.rs`) — authenticates the *user* to a *registry*. An auth-boundary check, not a supply-chain check.

**The macOS code-signing line in `quality-security.md` is inapplicable, not merely unverified.** Skills/rules/agents are markdown; MCP descriptors are TOML/JSON; bundles are a members list. None are Mach-O binaries.

**The only cosign mention in the repo** is `adr_hooks_support.md` — **Status: Proposed**. A design note for a hypothetical 5th artifact kind ("hooks": distributable executable scripts) listing Sigstore as a candidate mitigation *if* grim ever distributes self-executing code. Forward-looking awareness, not present capability.

**One live execution-adjacent surface that does exist and belongs in scope:** an `mcp` artifact's descriptor (`src/oci/mcp.rs`) carries `command` + `args` for the stdio transport. `grim add`/`install` writes that command into the client's own MCP config verbatim from whatever the registry served. grim never executes it — but it configures the *client* to launch it. That is an already-shipped path from registry content to a process the user's agent will run, independent of the no-signing gap.

**Recommended wording pattern (after uv):**
> grim pins every artifact by content digest (SHA-256) in the lockfile. That proves the bytes you install are exactly the bytes you locked and have not changed since. It does not prove who published them. grim does not verify cryptographic signatures on registry content — this is not an oversight to be silently fixed, it is the current scope.

---

## Q3 — Attack surfaces: guarded vs. unguarded vs. stale

### Real and guarded (verified against source)

| Surface | Evidence |
|---|---|
| **Registry authentication** | `src/auth/` — `GRIM_AUTH_<REGISTRY>_*` → docker credential store; `auth/verify.rs` verifies against the real challenge before storing; secrets flow only into the `Authorization` header (comment cites CWE-532) |
| **TLS / plain-HTTP opt-in** | `oci/access/registry_client.rs` — `GRIM_INSECURE_REGISTRIES` is an explicit allowlist, defaults localhost only, tested |
| **Digest verification (integrity, not authenticity)** | `oci/digest.rs`, `access.rs:84`, `cached_access.rs` |
| **Symlink escape from anchors** | Two-layer guard, both tested: `path_safety.rs::contain` (Layer 1 rejects `..`/root/prefix pre-filesystem; Layer 2 canonicalizes and asserts `starts_with`) and `install/path_anchor.rs::AnchoredPath::resolve` |
| **Zip-slip in tar** | `install/materializer.rs::safe_relative_path` — rejects `..`/absolute/root before anything touches disk; shared by on-disk and in-memory unpack paths |
| **Symlink injection in archives** | Stronger than prevented — any tar entry whose type is not `Dir`/`Regular` is refused outright |
| **Unbounded blob download (CWE-770)** | `adr_fetch_service_extraction.md` — streamed `CappedSink` aborts past `max_bytes` before the digest re-hash, plus a pre-download gate per caller |
| **MCP config writes** | Design-level review only. Per `subsystem-file-structure.md` these are AST-based splices (`toml_edit` / span-preserving JSON), not string concatenation. **A line-by-line pass on `json_splice.rs`/`toml_splice.rs` is recommended before SECURITY.md makes a specific claim here.** |

### Real and unguarded / only plausibly guarded

| Surface | Finding |
|---|---|
| **Windows junction points** | No junction-specific code or test (`junction`/`reparse`: zero hits). `dunce::canonicalize` plausibly resolves NTFS junctions like symlinks, so Layer 2 probably catches it — but every symlink-escape test is `#[cfg(unix)]`. **Untested on the platform it names.** |
| **MCP `command`/`args` as an execution vector** | Real, shipped, absent from the checklist entirely (Q2). |

### Stale — no longer match the code

| Bullet as written | What is actually true |
|---|---|
| **"Manifest signature validation"** | Does not exist. Highest-stakes drift — the internal checklist claims a control that would be false to repeat publicly. |
| **"Code Signing (macOS) — ad-hoc signing on Mach-O binaries"** | No code anywhere; inapplicable to grim's artifact kinds. |
| **"`${installPath}` template expansion in `metadata.json` env vars"** | Zero hits for `installPath` anywhere in `src/`. No such mechanism. The real one is `${VAR}` *passthrough* in `oci/mcp.rs` — grim writes the literal string for the client's runtime to substitute; grim never expands it. |
| **"Decompression bombs (xz/gz resource limits)"** | Inapplicable as worded — the layer media type is **uncompressed** tar and `Cargo.toml` has no `flate2`/`gzip`/`zstd`. The real risk (oversized blob) is covered by the CWE-770 caps. |
| **"File permission preservation (setuid/setgid)"** | grim preserves **no** tar-header permissions — `materializer.rs` writes via plain `std::fs::write`, no `set_permissions` anywhere. Files land at default umask regardless of header mode. No setuid risk exists because header bits are never applied. |
| **"Back-reference integrity … prevent GC"** | No garbage-collection command exists. `install/prune.rs` prunes client-side orphaned outputs; the content store is append-only with no shipped reclaim path. |

**Net effect:** `quality-security.md` needs a correction pass *before* SECURITY.md cites it.

---

## Q4 — Mechanics

**Security settings, re-checked with authenticated `gh` (orchestrator, 2026-07-26).** The researcher's unauthenticated fetch could not see `security_and_analysis` at all; authenticated it returns:

```
dependabot_security_updates            disabled
secret_scanning                        disabled
secret_scanning_push_protection        disabled
secret_scanning_non_provider_patterns  disabled
secret_scanning_validity_checks        disabled
private-vulnerability-reporting        {"enabled": false}
```

This confirms D4/D8 rather than merely echoing them. All five are W3-B scope.

**`grimoire-rs/.github` does not exist** — `gh api repos/grimoire-rs/.github` → 404. So the org-default pattern (uv/astral-sh) is unavailable without creating a new repo. **Decision: `SECURITY.md` goes in `grimoire`'s repo root**, which is also where `README.md:75` already links it and what 5/5 peers do at the repo level. Creating `grimoire-rs/.github` to cover all four repos stays open as a later, additive move.

**`has_discussions: true`** — corroborates W3-B's note that Discussions were enabled by hand.

**GitHub's file-discovery precedence**, quoted from GitHub's own "Creating a default community health file":
> "GitHub will use and display default files for any repository owned by the account that does not contain its own file of that type in the following order: **The `.github` folder, The root of the repository, The `docs` folder.**"

**The "Report a vulnerability" button and SECURITY.md are independent.** The button is gated solely by the private-vulnerability-reporting toggle; a SECURITY.md is not a prerequisite, and enabling the toggle without one still shows the button.

**`README.md:75` reads `- [Security Policy](SECURITY.md)` and the file does not exist** — a live 404. The meta-plan cited line 69 and D4 cited line 72; both were right when written. This checkout is under concurrent multi-agent edit, so **treat every cited line number as approximate until the file is opened.** `CODE_OF_CONDUCT.md` and `CONTRIBUTING.md` contain no other dangling security reference.

---

## Sources

| Source | Type | Date | Covers |
|---|---|---|---|
| `raw.githubusercontent.com/oras-project/oras/main/SECURITY.md` + `oras.land/docs/community/reporting_security_concerns` | Repo + docs | 2026-07-26 | Q1 |
| `github.com/sigstore/cosign/security/policy` | Repo | 2026-07-26 | Q1 |
| `raw.githubusercontent.com/astral-sh/uv/main/SECURITY.md` + `astral-sh/.github/main/SECURITY.md` | Repo ×2 | 2026-07-26 | Q1, Q2 wording model |
| `raw.githubusercontent.com/axodotdev/cargo-dist/main/SECURITY.md` | Repo | 2026-07-26 | Q1 |
| `raw.githubusercontent.com/helm/helm/main/SECURITY.md` + `helm/community/master/SECURITY.md` | Repo ×2 | 2026-07-26 | Q1 |
| local checkout `~/dev/grimoire` (mid-edit by concurrent agents) | Repo | 2026-07-26 | Q2, Q3 |
| `adr_fetch_service_extraction.md`, `adr_install_state_portability.md`, `adr_mcp_percall_scope_fetch_render.md`, `adr_hooks_support.md`, `adr_git_provenance_annotations.md` | ADRs | 2026-07-26 | Q2, Q3 |
| `gh api repos/grimoire-rs/grimoire --jq .security_and_analysis` (authenticated) | API | 2026-07-26 | Q4 — all five settings disabled |
| `gh api repos/grimoire-rs/grimoire/private-vulnerability-reporting` | API | 2026-07-26 | Q4 — `{"enabled":false}` |
| `gh api repos/grimoire-rs/.github` | API | 2026-07-26 | Q4 — 404, no org-default repo |
| `docs.github.com/.../creating-a-default-community-health-file` | Docs | 2026-07-26 | Q4 — precedence |
| `docs.github.com/.../configuring-private-vulnerability-reporting-for-a-repository` | Docs | 2026-07-26 | Q4 — button independence |
