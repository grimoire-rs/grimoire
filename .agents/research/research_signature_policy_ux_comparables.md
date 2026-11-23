# Research: Signature policy and UX in comparable tools

<!--
Technology Landscape Research
Filename: .agents/research/research_signature_policy_ux_comparables.md
Owner: Researcher (hex research phase)
Handoff to: Architect (/hex-architect), /hex-plan
-->

## Metadata

**Date:** 2026-11-17
**Domain:** security
**Triggered by:** Designing one fetch/resolve-time verification gate and a strictness policy setting for grim, under the rule that a repo must not grant itself trust on a cloner's behalf (CWE-426).
**Expires:** 2027-05-17 (Homebrew, Docker, uv, Argo CD and Claude Code plugin signing are all moving)

## Direct Answer

Findings only, no recommendation. Tools split into four patterns. (1) Opt-in per object: Helm `--verify`, Flux `spec.verify`, npm `audit signatures`, Homebrew `HOMEBREW_VERIFY_ATTESTATIONS`, Docker `DOCKER_CONTENT_TRUST=1`. (2) Verify by default with a key set shipped in the client: Nix `require-sigs=true` with `cache.nixos.org` key, VS Code marketplace signatures, Terraform provider checks, Go checksum DB. (3) Central policy objects in a cluster, owned by the operator not the app: Kyverno `verifyImages`, sigstore policy-controller `ClusterImagePolicy`, Ratify. (4) Per-tool verification metadata declared in a shared registry/catalog, with only a disable switch for the user: aqua, mise. Trust-widening is gated by privilege: Nix `trusted-users` and `accept-flake-config=false`, Claude Code managed settings vs trust-dialog-gated repo settings, Flux/Kyverno identities living in cluster-admin objects. Lockfile recording of signer identity is rare: mise records provenance (and warns it is not proof of verification), Go records hashes backed by a transparency DB, Terraform records hashes; none of the surveyed package managers record a signer identity as a pin except mise's provenance field. Installers (pip, uv, cargo) do not verify at install; npm/PyPI verify out-of-band.

## Comparison table

| Tool | Default | Identity location | Strictness vocabulary | Lock interaction | Publish side |
|---|---|---|---|---|---|
| Helm | opt-in `--verify` | local PGP keyring (`--keyring`) | none; flag only | no lock mention; `.prov` carries digest | `helm package --sign`; OCI via external plugin |
| Flux OCIRepository | opt-in `spec.verify` | `secretRef` (pubkeys / notation trustpolicy) or `matchOIDCIdentity` issuer+subject regex, in cluster object | none; set = enforce; fail => `SourceVerified=False`, `Ready=False`, backoff retry | n/a (digest in status) | external (cosign / notation) |
| Kyverno | per policy | `attestors.entries` keys/cert/keyless in ClusterPolicy | `Enforce` / `Audit`, `count`, `required`, `skipImageReferences`, `failurePolicy` | `mutateDigest` default true rewrites tag to digest | external |
| sigstore policy-controller | namespace opt-in via label `policy.sigstore.dev/include` | `ClusterImagePolicy` authorities key/keyless | `mode: warn` vs enforce (default enforce); `no-match-policy` warn/allow/deny (deny default) | n/a | external |
| Argo CD | opt-in GPG, deprecated | keys in cluster config + project | n/a verified | n/a | git commit signing |
| npm | install does not verify; `npm audit signatures` separate | registry keys at `/-/npm/v1/keys` | none | install first; no pin | `--provenance` in CI |
| Homebrew | opt-in env (beta) | hardcoded Homebrew identity; tap's own Actions identity | env on / `NO_VERIFY` off | none | CI attestation |
| PyPI / pip / uv | no install-time verify | attestation identities in pylock.toml (not enforced) | none | pylock.toml records identities | default with Trusted Publishing |
| cargo | no signing; trusted publishing only | n/a | n/a | n/a | OIDC token, no signature |
| Go | verify by default via sumdb | `GOSUMDB` (default sum.golang.org); unverified here | `GONOSUMDB`/`GOPRIVATE` opt-out patterns (from general knowledge) | `go.sum` hashes, first-use checked against DB | none |
| Nix | `require-sigs=true` | `trusted-public-keys` (default cache.nixos.org) | `require-sigs`; `trusted-users` | content-addressed exempt | `secret-key-files` |
| mise / aqua | on when tool declares it | per-tool in registry (aqua `registry.yaml`) or backend | aqua `-disable-cosign/-slsa`; mise `locked_verify_provenance` default off | mise lock records provenance + checksum | tool author |
| Docker DCT | off, env `DOCKER_CONTENT_TRUST=1` | Notary root key (offline) per repo | on/off; digest pulls bypass | none | `docker trust sign` (retiring) |
| VS Code | verify on (`extensions.verifySignature`) | marketplace signs all | publisher-trust dialog separate | none | marketplace |
| Terraform | verify by default (registry GPG) | key published in registry | `dev_overrides` bypass | `.terraform.lock.hcl` hashes | GPG signed release |
| MCP Registry | namespace ownership proof | reverse-DNS: GitHub OAuth/OIDC, DNS, HTTP | n/a | n/a | `mcp-publisher` |
| Claude Code plugins | no signatures | n/a; marketplace source allowlist | `strictKnownMarketplaces`, `blockedMarketplaces` | none (signing issue closed not planned) | n/a |

## Key Findings

### Pattern A: opt-in, one switch
- Flux verification only runs when `spec.verify` is set; failure flips `SourceVerified=False` and retries with backoff. https://fluxcd.io/flux/components/source/ocirepositories/
- Helm `--verify` aborts the install before render on missing/corrupt `.prov`, unknown key, or hash mismatch; keyring path via `--keyring`. https://helm.sh/docs/topics/provenance/ (old doc, likely over 18 months)
- npm: `npm audit signatures` is a separate command; reports packages missing signatures where the registry provides keys. https://docs.npmjs.com/verifying-registry-signatures ; provenance verification also through audit. https://docs.npmjs.com/generating-provenance-statements
- Homebrew: not default; `HOMEBREW_VERIFY_ATTESTATIONS` enables; "hope to make default" once pure Ruby. https://docs.brew.sh/Homebrew-Security-and-Supply-Chain . Developer-mode users hit unexpected failures and used `HOMEBREW_NO_VERIFY_ATTESTATIONS`. https://til.simonwillison.net/homebrew/no-verify-attestations
- Docker: content trust off by default; `DOCKER_CONTENT_TRUST=1` blocks unsigned tags; pull by digest bypasses. https://docs.docker.com/engine/security/trust/

### Pattern B: on by default with shipped key set
- Nix: `require-sigs` true; `trusted-public-keys` defaults to cache.nixos.org key; content-addressed paths exempt. https://nix.dev/manual/nix/stable/command-ref/conf-file.html
- VS Code: marketplace signs, client verifies at install; failure blocks with "cannot verify the extension signature"; disable via `extensions.verifySignature`. Separate first-time publisher-trust dialog (1.97+). https://code.visualstudio.com/docs/configure/extensions/extension-marketplace
- Terraform: lock file hashes enforced; `dev_overrides` disables checks. https://developer.hashicorp.com/terraform/cli/config/config-file (provider GPG-in-registry claim NOT verified here)
- Verified publisher is not a trust boundary: tampered build shipped under a verified publisher account for about eleven hours (secondary source). https://pub.towardsai.net/verified-publisher-is-not-a-trust-boundary-what-the-trojanized-claude-code-extension-teaches-d8c5b2395dd0

### Pattern C: central policy vocabulary (admission)
- Kyverno: identities in `attestors[].entries` (keys, certs, keyless subject/issuer or regex), `count`, Enforce, `skipImageReferences`, `failurePolicy: Ignore` for registry outage; unsigned gives "signature not found", wrong signer "invalid signature"; `mutateDigest` defaults true. https://release-1-18-0.kyverno.io/docs/policy-types/cluster-policy/verify-images/sigstore/
- policy-controller: namespace label opt-in; CIPs ANDed, authorities ORed; `mode: warn`; unmatched images denied unless `no-match-policy` set. https://docs.sigstore.dev/policy-controller/overview/
- Ratify: verification engine, policies user-created; detail pages not read. https://ratify.dev/docs/what-is-ratify

### Pattern D: verification metadata declared by catalog, user only disables
- aqua: cosign/SLSA settings live in the registry definition; verify runs automatically when declared; user disable via `-disable-cosign`, `AQUA_DISABLE_COSIGN`, rationale: Sigstore endpoints blocked on some networks; failure stops install. https://aquaproj.github.io/docs/reference/security/cosign-slsa/
- mise: lock records provenance; `mise lock` fails if artifact changed and provenance cannot be re-established; recorded provenance "not proof that mise verified the bytes"; install may skip repeat provenance check when checksum present, `locked_verify_provenance` default off. https://mise.jdx.dev/dev-tools/mise-lock.html
- `gh attestation verify` requires `--owner` or `--repo`; supports `--signer-workflow`, `--cert-identity(-regex)`, offline `--bundle`; repo name check on bundles cannot detect renames. https://cli.github.com/manual/gh_attestation_verify

### Pattern E: who may widen trust
- Nix: `trusted-users` equals root; `accept-flake-config` default false (flake-carried config needs consent). https://nix.dev/manual/nix/stable/command-ref/conf-file.html
- Claude Code: repo `.claude/settings.json` `extraKnownMarketplaces` ignored silently until workspace trust accepted; managed settings override all scopes; `strictKnownMarketplaces` matches marketplace source, not entries inside; `hostPattern` regex. https://code.claude.com/docs/en/plugins/org.md
- Flux/Kyverno/policy-controller: identity declared in cluster objects needing admin RBAC (inference from the objects' scope, not a quoted claim).

### Lockfile interaction
- mise (above); Terraform lock hashes with cache bypass flag; Flux n/a; PyPI pylock.toml carries attestation identities but installers do not check them (secondary). https://pydevtools.com/handbook/explanation/why-pylock-toml-includes-digital-attestations/ ; https://discuss.python.org/t/pypis-new-security-indicators-and-unequal-access-to-trusted-publishing/109930

### Publish side and ecosystem state
- PyPI: attestations generated by default under Trusted Publishing; uv `publish` behavior conflicting in sources. https://blog.pypi.org/posts/2024-11-14-pypi-now-supports-digital-attestations/
- cargo: trusted publishing (RFC 3691) shipped, signing RFCs unresolved. https://rust-lang.github.io/rfcs/3691-trusted-publishing-cratesio.html ; https://internals.rust-lang.org/t/pre-rfc-using-sigstore-for-signing-and-verifying-crates/18115
- MCP Registry: namespace proven by GitHub OAuth/OIDC, DNS, or HTTP; no artifact signature. https://modelcontextprotocol.io/registry/about

### Post-mortems and complaints
- Docker Content Trust retired: Notary v1 shuts down 2026-12-08; under 0.05% of Hub pulls used it; brownouts; affected: env var users, `docker trust`, K8s admission checking DCT. https://www.docker.com/blog/docker-content-trust-retirement-and-migration-guidance/ ; losing non-root keys needs Hub support and manual consumer intervention. https://docs.docker.com/engine/security/trust/
- Claude Code plugin signing request: content-hash pinning minimum, publisher signatures fuller; no maintainer reply, closed not planned 2026-04-30, locked 2026-06-19. https://github.com/anthropics/claude-code/issues/29729
- Homebrew attestation beta surprised developer-mode users (see above).

## Negative

- Argo CD GPG page returned only a deprecation stub (replaced by "Source Integrity Verification"); its policy model is unread.
- Go sumdb semantics (GOSUMDB default, GONOSUMDB, mismatch text) not read from the doc fetched (page truncated); table entry is from general knowledge, unverified.
- Terraform registry GPG-key claim, Bazel/bzlmod, JetBrains, apt/dnf, ORAS, Smithery, skills.sh not researched.
- Kyverno `Audit` mode semantics not covered by the fetched page; Ratify config model unread.
- Docker Scout default and any default-on backlash or key rotation incident: nothing found.
- Homebrew current default after 7.0.0 unconfirmed; "no installer verifies PEP 740 at install" is from secondary guides.
- Claude Code plugin docs found no signature verification; trusted-source gating only.

## Leads

- Source integrity verification (Argo CD) — successor to GPG verify may carry a newer policy model.
- Go sumdb / TUF — transparency-log alternative to identity pinning, full read of the checksum database doc.
- Bazel bzlmod registry integrity — lockfile and registry-hosted trust.
- Homebrew `attestation.rb` source — exact failure text, exit behaviour, default flip history.
- Notation trust policy format — scopes, `trustStores`, `signatureVerification` levels (strict/permissive/audit/skip) as policy vocabulary.

## Sources

| Source | Type | Date | Relevance |
|---|---|---|---|
| https://fluxcd.io/flux/components/source/ocirepositories/ | Docs | current | Flux verify |
| https://release-1-18-0.kyverno.io/docs/policy-types/cluster-policy/verify-images/sigstore/ | Docs | 1.18 | Kyverno |
| https://docs.sigstore.dev/policy-controller/overview/ | Docs | current | policy-controller |
| https://docs.npmjs.com/verifying-registry-signatures | Docs | undated, possibly old | npm |
| https://mise.jdx.dev/dev-tools/mise-lock.html | Docs | 2026 | mise lock |
| https://docs.docker.com/engine/security/trust/ | Docs | 2026 | DCT |
| https://www.docker.com/blog/docker-content-trust-retirement-and-migration-guidance/ | Blog | 2026 | retirement |
| https://docs.brew.sh/Homebrew-Security-and-Supply-Chain | Docs | undated | Homebrew |
| https://nix.dev/manual/nix/stable/command-ref/conf-file.html | Docs | current | Nix |
| https://code.claude.com/docs/en/plugins/org.md | Docs | 2026 | managed trust |
| https://github.com/anthropics/claude-code/issues/29729 | Issue | 2026 | plugin signing |
| https://code.visualstudio.com/docs/configure/extensions/extension-marketplace | Docs | current | VS Code |
| https://aquaproj.github.io/docs/reference/security/cosign-slsa/ | Docs | possibly old | aqua |
| https://cli.github.com/manual/gh_attestation_verify | Docs | current | gh |
| https://helm.sh/docs/topics/provenance/ | Docs | likely older than 18 months | Helm |
| https://modelcontextprotocol.io/registry/about | Docs | 2025 | MCP registry |
