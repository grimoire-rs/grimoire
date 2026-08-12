# Security Policy

## Reporting a vulnerability

Report privately through GitHub's [private vulnerability reporting][pvr] —
the **Report a vulnerability** button on the [Security tab][security-tab].
That keeps the report and the fix in one place and lets us credit you when it
lands.

Please do not open a public issue, discussion, or pull request for a suspected
vulnerability, and do not post it to social media before there is a fix.

Grimoire is maintained by one person. Expect a first response within about a
week; there is no formal SLA, and inventing one nobody can hold to would be
worse than saying this. If a report goes unanswered for two weeks, feel free to
nudge it.

If you are unsure whether something is a vulnerability, report it anyway.
Over-communication is welcome — a duplicate costs a few minutes, a missed
report costs users.

## Supported versions

The latest release. Fixes land on `main` and go out in the next release. There
is no backport branch, and older versions receive no patches — pin a version if
you need reproducibility, but expect to move forward for a fix.

## What grim guarantees, and what it does not

Read this before deciding whether something is a vulnerability. grim installs
other people's configuration onto your machine, so its trust boundaries are
worth stating explicitly.

**grim verifies integrity, not authenticity.** Every artifact is pinned by
SHA-256 content digest in `grimoire.lock`, and every downloaded blob is checked
against that digest. This proves the bytes you install are exactly the bytes
you locked and that nothing changed underneath you. It proves nothing about
*who* published them.

**grim does not verify signatures.** There is no cosign, notation, or sigstore
verification of registry content anywhere in the codebase. This is the current
scope, not an oversight awaiting a silent fix. Treat every registry you
configure the way you would treat a container registry you pull images from:
a trust boundary you are choosing to accept. The first `grim add` of a
reference trusts whatever the registry serves at that moment — exactly like
`docker pull` without `cosign verify`. If you want signature verification,
open a feature request; it is a design change, not a bug report.

**An installed `mcp` artifact configures a program your AI client will run.**
An MCP descriptor carries a `command` and `args` for the stdio transport, and
`grim install` writes them into your client's own MCP config file verbatim as
the registry supplied them. grim never executes that command — your AI client
does, on its next session. This is the one path where installing an artifact
leads to a process running, and it deserves the same scrutiny you would give
any `postinstall` script. Review MCP descriptors from registries you do not
control.

**These are not vulnerabilities in grim:**

- Malicious or hostile content in a registry you configured. Registry choice is
  yours; grim does not curate.
- An MCP descriptor whose `command` runs something you did not want, when it
  came from a registry you added.
- An artifact that installs successfully but instructs an agent to behave badly.
  grim distributes configuration; it does not evaluate what that configuration
  asks a model to do.
- A client reading configuration grim wrote correctly. Where a client has no
  surface that can host an artifact faithfully, grim declines and warns rather
  than writing something misleading — see the
  [client compatibility matrix][clients].

## In scope

These are the boundaries where a report is a genuine security issue:

- **Path escape.** Any artifact content that writes outside its intended
  directory — traversal in a tar entry, a symlink or junction escaping its
  anchor, a crafted path component defeating containment.
- **Credential exposure.** Registry credentials appearing in logs, error
  messages, reports, JSON output, or any file grim writes; or being sent to a
  host other than the registry they belong to.
- **Transport downgrade.** Any path that reaches a registry over plain HTTP
  without the host being explicitly opted in — either listed in
  `GRIM_INSECURE_REGISTRIES` or declared `insecure = true` on its own
  `[[registries]]` entry. Both are deliberate user opt-ins and are not
  themselves vulnerabilities; that a committed `grimoire.toml` carries the
  config-file opt-in to everyone who clones the project is documented
  behaviour, not a finding. The opt-in covers **transport to that host
  only**: it must never widen where a credential may be sent, so a registry
  reached over HTTPS answering with a plaintext `Bearer` realm is a finding
  regardless of which hosts are opted in.
- **Digest bypass.** Content installed without its digest being checked, or a
  mismatch that does not abort.
- **Config corruption.** grim writing a client's MCP or rule configuration in a
  way that damages unrelated entries, or that injects content through a value
  that should have been quoted or escaped.
- **Resource exhaustion.** A registry response that makes grim consume
  unbounded memory or disk despite the download size caps.

## Known limitations

Not vulnerabilities, but worth knowing:

- **Windows junction points are untested.** The path-containment guard
  canonicalizes before asserting containment, which should resolve NTFS
  junctions the same way it resolves symlinks — but the escape tests are
  Unix-only. If you can demonstrate an escape on Windows, that is very much in
  scope.
- **Tar-header permission bits are discarded.** Extraction never applies them,
  so installed files take your umask. No setuid surface exists, and no
  executable bit survives packaging.
- **The content store is append-only** with no reclaim path. Disk use grows
  until you delete `$GRIM_HOME` by hand.

<!-- external -->
[pvr]: https://docs.github.com/en/code-security/security-advisories/guidance-on-reporting-and-writing-information-about-vulnerabilities/privately-reporting-a-security-vulnerability
[security-tab]: https://github.com/grimoire-rs/grimoire/security

<!-- internal -->
[clients]: https://grimoire.rs/clients.html
