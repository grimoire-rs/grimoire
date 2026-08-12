---
paths:
  - ".github/workflows/**"
  - ".github/actions/**"
  - ".github/dependabot.yml"
---

# Security Standards

Deep-dive reference for security reviews. See Core Principle 3 ("Keep It Safe") in AGENTS.md for essentials.

## Security Checklist

- [ ] No hardcoded secrets or credentials
- [ ] All user input validated + sanitized
- [ ] SQL queries use parameterized statements
- [ ] Auth + authorization properly implemented
- [ ] Sensitive data encrypted at rest + in transit
- [ ] Error messages no expose internal details
- [ ] Dependencies up to date + vuln-free

## OWASP Top 10 2021

| Category | Check For |
|----------|-----------|
| Broken Access Control | Missing authorization checks |
| Cryptographic Failures | Unencrypted sensitive data |
| Injection | SQL, Command, XSS vulnerabilities |
| Insecure Design | Missing threat modeling |
| Security Misconfiguration | Default credentials, debug enabled |
| Vulnerable Components | Outdated/CVE-affected packages |
| Auth Failures | Weak passwords, session issues |
| Integrity Failures | Unsigned updates, untrusted deserialization |
| Logging Failures | Missing audit trails |
| SSRF | Unvalidated URLs in server requests |

## Severity Classification

| Severity | Definition | Action |
|----------|------------|--------|
| Critical | Exploitable vulnerability, data loss risk, high impact | MUST fix before merge |
| High | Exploitable vulnerability, breaking change, moderate impact, major bug | MUST fix before merge |
| Medium | Requires conditions to exploit, performance issue, code smell | SHOULD fix, can negotiate |
| Low | Best practice violation, style, minor improvement | COULD fix, optional |

## CWE References

Reference CWE (Common Weakness Enumeration) IDs for standardized vuln classification. Example: `CWE-89` for SQL Injection, `CWE-798` for hardcoded credentials.

## Dependency Safety

- Warn on deprecated/vulnerable deps
- Audit new deps before adding
- Keep deps updated
- Use automated scanning (Trivy, Snyk, Dependabot)

## Output Guidelines

- Never expose actual secrets in analysis output
- Give specific file locations + line numbers
- Include concrete remediation steps
- Check code AND config files

## Grimoire-Specific Attack Surfaces

Recurring attack surfaces in Grimoire codebase. Use as STRIDE scoping checklist for any Grimoire audit.

### Registry Authentication
- Auth chain: `GRIM_AUTH_<REGISTRY>_*` env vars → Docker credentials (`~/.docker/config.json`)
- Credentials never logged or in error messages
- Plain HTTP (`GRIM_INSECURE_REGISTRIES`, or `insecure = true` on a
  `[[registries]]` entry) only for localhost/in-cluster/test registries. The
  config-file form travels with a committed `grimoire.toml` — it downgrades
  transport for every collaborator, not just the author

### Registry Communication
- TLS verification for all registry connections (except insecure registries)
- Digest verification on downloaded content (SHA-256 match) — this is
  **integrity, not authenticity**: it proves the bytes match the lock, never
  who published them
- **No signature verification exists.** No cosign, notation, or sigstore
  anywhere in `src/`. Do not audit for it and never claim it. Signing is
  named only in `adr_hooks_support.md`, which is *Proposed*, for a kind that
  does not ship

### Path Containment
- Install paths no escape their anchor via traversal — two-layer guard
  (`path_safety.rs::contain`, `install/path_anchor.rs::AnchoredPath::resolve`):
  Layer 1 rejects `..`/root/prefix components before touching the filesystem,
  Layer 2 canonicalizes and asserts `starts_with`
- Windows junction points: **unverified.** `dunce::canonicalize` plausibly
  resolves NTFS junctions like symlinks, but every escape test is
  `#[cfg(unix)]` — untested on the platform it names

### Archive Extraction
- Path traversal in tar archives (zip slip) — `materializer.rs::safe_relative_path`
- Symlink injection: any tar entry not `Dir`/`Regular` is refused outright
- Tar-header permission bits are **never applied** — extraction is plain
  `std::fs::write`, so files land at the default umask. There is no
  setuid/setgid surface to guard
- Oversized blobs (CWE-770) — a streamed `CappedSink` aborts past `max_bytes`
  before the digest re-hash, plus a per-caller pre-download gate. Compression
  bombs do not apply: the layer media type is uncompressed tar and there is no
  `flate2`/`gzip`/`zstd` dependency

### MCP Descriptor Execution
- An `mcp` artifact carries `command` + `args` for the stdio transport. Install
  writes them into the client's own MCP config **verbatim from the registry**.
  grim never executes them — the AI client does, on its next session. This is
  the one shipped path from registry content to a running process
- `${VAR}` in a descriptor is passthrough, not expansion: grim writes the
  literal string for the client's runtime to substitute, and never interprets
  it

<!-- Corrected 2026-07-26 against source (research_security_policy.md, Q3).
     Removed: manifest signature validation, macOS Mach-O code signing,
     `${installPath}` env templating, decompression bombs, setuid preservation,
     and back-reference/GC integrity — none of these describe shipped code.
     A stale checklist entry is worse than a missing one: it gets repeated into
     a public document as a control that does not exist. -->

### Content Store
- Append-only and immutable; no garbage-collection command exists.
  `install/prune.rs` prunes **client-side orphaned outputs**, not store content

## Grimoire Audit Checklist

- [ ] Auth/authorization flow
- [ ] Input validation (identifiers, tags, paths)
- [ ] Secrets management (no credentials in logs/errors)
- [ ] Dependency vulnerabilities (`trivy` scan)
- [ ] Archive extraction safety
- [ ] Symlink traversal prevention
- [ ] Env var injection