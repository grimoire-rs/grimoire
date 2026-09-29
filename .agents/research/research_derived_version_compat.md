# Research: content-derived versions and additive schema evolution for generated plugin artifacts

## Metadata
Date: 2026-09-28 · Expires: 2027-03-28 · Axis: data model / compatibility · Triggered by: /hex-architect .agents/discussions/marketplace-phase-2.md

All web sources accessed 2026-09-28. Fetched pages are data. Claims marked *inference* are mine, not a source's. Local refs are `file:line`.

## Direct Answer

- Mature build systems key on **declared inputs plus an explicit toolchain identity**, not on the tool's release number. Cargo, Nix and Bazel keep the toolchain in the key and pay for it with over-invalidation. A cache miss is cheap and invisible there. In grim it is a user-facing "update available" prompt, so option (c) does not transfer.
- Where an identifier must outlive tool releases, the precedents put a **scheme tag inside the identifier or record** (Go/Terraform `h1:`, Cargo lock `version`). They then bump it deliberately. That is option (b), and grim already has a design for it (`render_scheme`, below).
- Option (a) (hash rendered output) has strong self-reference precedent (wheel `RECORD`, Nix content-addressing). Its cost is that determinism becomes load-bearing, and the version becomes per-client.
- Adding an optional table to a `deny_unknown_fields` file is **backward-compatible but not forward-compatible**. grim's own stability page already accepts that trade (stability.md:326-362). Cargo's reader-first, writer-later gating (V4 introduced 1.78, default 1.83) is the closest precedent for making it safe.

## Local baseline (what is actually frozen)

| Fact | Source |
|---|---|
| Suffix = first 12 hex of sha256 over sorted `kind\temitted name\tcontent digest` lines; client-independent | `.agents/adr/adr_harness_plugin_export.md:315-321` |
| Description edits and renderer changes yield new bytes under the same version; accepted risk, mitigation is "bump `version`" | `adr_harness_plugin_export.md:322-326`, `:489` |
| ADR D6 calls **grammar and hash input** one-way "(a change fires a phantom update in every harness)" | `adr_harness_plugin_export.md:337-338` |
| ADR D11 lists "version grammar + hash input" as new frozen contracts, frozen from 1.0 | `adr_harness_plugin_export.md:430` |
| stability.md freezes the **grammar** `<base>+<12-hex>` and the report shape only; member bytes are explicitly unstable and "a minor release ... changes an exported plugin's bytes identically, without bumping the plugin's own `version`" | `docs/src/content/docs/stability.md:33`, `:195-205` |
| Manifest inputs may widen (new optional key, value or spelling) but never narrow | `stability.md:118-121` |
| Lock/state files reject unknown fields on purpose; older grim exits 78 on newer-feature files; "a deliberate departure from the ecosystem norm" | `stability.md:326-362` |
| A per-vendor `render_scheme: u16` stamp (0 = pre-versioning, bumped only when output shape changes, `skip_serializing_if` zero so old files stay byte-identical) is designed but deferred | `.agents/specs/design_render_scheme_versioning.md` §4 Tier 1 |
| Consequence: ADR D11 and stability.md disagree on whether "hash input" is frozen. The docs promise less than the ADR. | `adr_harness_plugin_export.md:430` vs `stability.md:33` |

## Q1. Precedents that react to toolchain changes

**How consumers actually compare (why it matters).** Claude Code computes a version per plugin and uses it to detect updates; it "skip[s] the plugin when it matches what `installed_plugins.json` records" ([loading](https://code.claude.com/docs/en/plugins/loading#versions-and-updates)). A manifest `version` comes first, and "pins the plugin to that version until you change it" ([manifest ref](https://code.claude.com/docs/en/plugins/manifest-reference)). The version is "not checked against semver" (same page). So grim's embedded version is what suppresses the harness's own change detection. When no `version` is set, Claude Code derives one itself: commit SHA (12 chars) for git sources, sha256 (12 chars) for `archive`, and for `command` sources a hash of the copied files, or `<manifest version>-<hash>` ([loading](https://code.claude.com/docs/en/plugins/loading#how-claude-code-computes-the-version); [marketplace ref](https://code.claude.com/docs/en/plugins/marketplace-reference#copy-mode-and-link-mode)). That last form is option (a) implemented by the harness, and it uses `-` (a pre-release), not `+`.

| Option | Precedent | Failure mode observed |
|---|---|---|
| (a) hash output, exclude self-reference | Nix content-addressed outputs; Go/Terraform `h1` content hash; wheel `RECORD` (Q2) | Needs a deterministic canonical form. GitHub's archive change altered bytes with unchanged inputs and broke every pinned hash ([GitHub blog](https://github.blog/open-source/git/update-on-the-future-stability-of-source-code-archives-and-hashes/): the "byte layout of the archive itself changed"). |
| (b) explicit scheme constant | Go `h1:` (`DefaultHash` = `Hash1`, [dirhash](https://pkg.go.dev/golang.org/x/mod/sumdb/dirhash)); Terraform: "We may occasionally introduce new hashing schemes" ([lock file](https://developer.hashicorp.com/terraform/language/files/dependency-lock)); Cargo lock `version` | Human must remember to bump: a missed bump is a missed update. No mature system relies on memory alone. *inference:* they pair the tag with tests. |
| (c) tool version in key | Cargo fingerprint includes "rustc version", profile, features, deps, and excludes mtime and absolute paths ([fingerprint docs](https://doc.rust-lang.org/stable/nightly-rustc/cargo/core/compiler/fingerprint/index.html)); Nix input-addressed path depends on "the system and the builder executable" and "every time we change the derivation, a new hash is created" ([Nix pills](https://nixos.org/guides/nix-pills/06-our-first-derivation.html)) | Over-invalidation on every toolchain release. Cargo states fingerprinting is "imperfect" and captures only "a small part of the environment". Tolerable because a rebuild is silent. |
| (d) leave as is | GitHub (pre-2023) said archive checksums were never guaranteed | Consumers built on the stability anyway; GitHub then committed to "byte-for-byte stable for no less than a year" plus "six months' notice" (same blog). |

Missed-invalidation evidence for leaving the toolchain out of the key:
- babel-jest did not vary its cache by Babel version, so a downgrade kept stale output and needed `--no-cache` ([jest#7584](https://github.com/jestjs/jest/issues/7584)).
- Bazel warns that untracked tools mean "two users with different compilers installed will wrongly share cache hits" ([Bazel remote caching](https://bazel.build/remote/caching)).
- Bazel's action key is "a hash of non-file data ... such as its command line arguments and mnemonic" ([EngFlow](https://blog.engflow.com/2024/05/13/the-many-caches-of-bazel/), via search). Note it is the command line, not the Bazel release number.

Reproducible builds frame the goal: output should be a function of source plus a declared build environment, and independent of incidental facts like build time ([SOURCE_DATE_EPOCH](https://reproducible-builds.org/docs/source-date-epoch/)). Tool release number is incidental unless it changes output.

*Inference for grim:*
- (c) would make two teammates on different grim patch versions export different versions for identical content. In a shared marketplace repo that means commit churn and phantom prompts at every grim release (v0.14.3 is the current cadence).
- (c) is the only option with both false updates and mismatch between machines.
- (b) has no false updates by construction, and its only failure is a missed bump.
- (a) has neither, provided the render is byte-deterministic across OSes. ADR D8 records that directory outputs are not normalized (`phase1-discover.md` "Directory output modes", risk 8).

## Q2. Hashing a tree that contains its own version

| Technique | Example | Note |
|---|---|---|
| Exclude the self-describing file | Wheel `RECORD` must list every installed file including itself, but "entries for ... the `RECORD` file itself have empty *hash* and *size*" ([PyPA spec](https://packaging.python.org/en/latest/specifications/recording-installed-packages/)) | Simplest; canonical form is "all files but the manifest". |
| Hash inputs before the build, never the output | Nix input-addressed: "the hash of the out path is based solely on the input derivations ... not on the contents of the build product" ([Nix pills](https://nixos.org/guides/nix-pills/06-our-first-derivation.html)) | This is what grim does today (member digests in, version out). It sidesteps circularity entirely. |
| Placeholder, hash, substitute | Nix CA outputs: build, "replace all the occurrences of a self-reference by a magic value", hash, then "replace ... the magic value by the final path" ([Tweag](https://www.tweag.io/blog/2020-11-18-nix-cas-self-references/)) | Works only because "all the self-references will appear textually". Fragile for binary or compressed content. |
| Hash content, not container | Go `h1:` = sha256 over sorted `"<sha256 of file>  <name>\n"` lines ([dirhash](https://pkg.go.dev/golang.org/x/mod/sumdb/dirhash)); Terraform `h1:` hashes the package "contents ... rather than of the `.zip` archive" ([lock file](https://developer.hashicorp.com/terraform/language/files/dependency-lock)) | Directly mirrors grim's current line format. Makes dir and zip outputs hash equal. |
| Record the hash outside the hashed object | Lockfiles (`go.sum`, Terraform lock) | The `marketplace.lock` already plays this role. |

*Inference for grim:* the plugin manifest is serialized from a fixed-order struct (`adr_harness_plugin_export.md:294-298`), and the generated README carries no version (`:330-334`), so "hash every file except `plugin.json`, plus `plugin.json` serialized with `version` omitted" is a clean canonical form. It is per `(plugin, client)` because `README.md` and translated `.mcp.json` differ per client (`phase1-discover.md` risk 1). `ExportReport` items already carry `client` and `version` per row, so the shape does not change (`phase1-discover.md` line 14).

## Q3. Build metadata and precedence: is `+hash` invisible anywhere?

- SemVer 2.0: "Build metadata MUST be ignored when determining version precedence. Thus two versions that differ only in the build metadata, have the same precedence." ([semver.org](https://semver.org/))
- Go ignores it when comparing ([go.dev/ref/mod](https://go.dev/ref/mod)). Cargo ignores it and says never publish versions differing only in metadata ([Cargo dependency docs](https://doc.rust-lang.org/cargo/reference/specifying-dependencies.html), via search). npm publish drops it ([npm/npm#6379](https://github.com/npm/npm/issues/6379), via search). Helm's semver comparison ignores it, and Helm rewrites `+` to `_` in OCI tags ([helm/helm#10250](https://github.com/helm/helm/issues/10250), via search).
- The VS Code Marketplace reportedly accepts only `X.Y.Z` ([vsce#871](https://github.com/microsoft/vscode-vsce/issues/871), via search; not verified against Microsoft docs).
- Harnesses: Claude Code is string-equality on the computed version (above; also `research_marketplace_lock_patterns.md:15`). Cursor's update is "git ref refresh, not version compare"; OpenClaw uses semver with "many versions per package"; Copilot and Codex comparison is undocumented (`research_plugin_support_matrix.md:45,48,55,82`).

Conclusion: `+hash` is invisible **only** to a semver-aware comparer. In this repo's evidence that is OpenClaw (and OpenCode, npm transport, `research_plugin_support_matrix.md:68`), and possibly Copilot/Codex (unknown). *Inference:* a `-hash` pre-release, as Claude Code itself emits, would be visible to semver comparers but has arbitrary lexical ordering, so a "new" hash can sort lower and read as a downgrade. `+` is right for string-equality consumers and wrong for semver ones. Neither choice fixes the other class. Only a real base bump reaches semver-aware harnesses, which is what `adr_harness_plugin_export.md:486-488` already records. Nothing found changes that trade.

## Q4. Additive evolution under a strict parser

| Tool | Mechanism | Older tool sees new file |
|---|---|---|
| Cargo manifest | Unknown keys warn ("unused manifest key"); `package.metadata` is "completely ignored" pass-through ([manifest](https://doc.rust-lang.org/cargo/reference/manifest.html)) | Warns, builds |
| Cargo `rust-version` | "Cargo will report that as an error ... avoids ... a less direct diagnostic like invalid syntax" ([rust-version](https://doc.rust-lang.org/cargo/reference/rust-version.html)) | Only tools new enough to know the key benefit |
| Go `go` directive | Since 1.21 "Go toolchains refuse to use modules declaring newer Go versions" ([go.dev/ref/mod](https://go.dev/ref/mod)) | Clear refusal from aware toolchains |
| Terraform `required_version` | "prints an error and exits" when unmet ([terraform block](https://developer.hashicorp.com/terraform/language/block/terraform)) | Same |
| Cargo.lock | Reader-first: V4 introduced in 1.78, default for new lockfiles only from 1.83; policy "not updated until at least the support for the version is in the stable release" ([ResolveVersion](https://doc.rust-lang.org/stable/nightly-rustc/cargo/core/resolver/enum.ResolveVersion.html)); older cargo hard-errors "lock file version `4` was found" ([cargo#10046](https://github.com/rust-lang/cargo/issues/10046)) | Hard error, but only after a deliberate gap |
| Claude Code | Unknown top-level `plugin.json` keys stripped; `userConfig`/`channels` entries strict ([manifest ref](https://code.claude.com/docs/en/plugins/manifest-reference)) | Mixed by design |

Compatibility definitions:
- Kubernetes: a change is compatible if it adds "functionality that is not required for correct behavior" and "existing clients need not be aware" ([api_changes.md](https://github.com/kubernetes/community/blob/master/contributors/devel/sig-architecture/api_changes.md)).
- Schema registries: with a closed content model (`additionalProperties: false`), adding a property is backward-compatible only; older readers reject it ([Confluent](https://docs.confluent.io/cloud/current/sr/fundamentals/schema-evolution.html), via search).
- grim's own definition is the input-widening rule at `stability.md:118-121`. Under it a new optional `[marketplace]` table is additive. The older-binary reject (exit 65) is the same accepted trade already documented for locks at `stability.md:326-362`.
- Cargo's `cargo-features` pattern (new syntax gated behind an explicit key that old tools flag) is unverified. The fetched summary claimed older Cargo only warns; I could not confirm that from primary text. Treat as a lead.

*Inference:*
- Adding `[marketplace]` is additive by grim's rule and by K8s' rule, and non-forward-compatible by the closed-model rule. All three are true at once, so the ADR should say "backward-compatible, not forward-compatible" instead of "additive".
- A pinned older grim gets exit 65 with no hint that upgrading fixes it. The tools above that ship a clear message (Cargo `rust-version`, Go, Terraform) all did so via a **version-gate key introduced before it was needed**. grim can still add such a reserved key (e.g. `min_grim_version`) while `marketplace.toml` is pre-1.0 and the top-level reserved-key mechanism exists (`phase1-discover.md` line 47). Old binaries reject the key itself, so it protects only the *next* widening.
- The Cargo V4 gap (reader in one release, writer default later) is the only precedent that helps the *current* widening. It costs nothing extra: ship a release that parses `[marketplace]` while nothing writes it, then let `grim export`/`update` emit it in a later release.

## Q5. What comparable projects call "breaking" for a derived identifier

| Project | Change | Treated as |
|---|---|---|
| Go | New hash algorithm | Ships as a **new scheme name** (`h1`), not an edit of the old one; `DefaultHash` selects it "for new go.sum entries" ([dirhash](https://pkg.go.dev/golang.org/x/mod/sumdb/dirhash)) |
| Terraform | New hash scheme | Additive: multiple hashes per version coexist; `h1:` added "opportunistically" beside `zh:` ([lock file](https://developer.hashicorp.com/terraform/language/files/dependency-lock)) |
| Cargo lock | v1 -> v3 -> v4 | New readers accept all old formats; old lockfiles keep their version unless rewritten ("also used for updated lock files"); default moves only after stable support ([ResolveVersion](https://doc.rust-lang.org/stable/nightly-rustc/cargo/core/resolver/enum.ResolveVersion.html)) |
| Nix | Any derivation input change | Not breaking, expected: a new hash is a new cache key (Nix pills, above) |
| GitHub archives | Compression change, same inputs | Documented as unguaranteed, treated as breaking by consumers, then promised stable for a year with six months' notice |
| Cargo fingerprint | Internal | Unstable by design; not an interface |

Pattern: an identifier is treated as a contract **when a consumer verifies or pins it** (`go.sum`, lockfiles, archive checksums). It is treated as a disposable cache key when the consumer only re-derives it (Nix, Cargo fingerprint).
- grim's version sits in between. Harnesses only compare it for change, nobody verifies it against an external hash. But its *value continuity* is user-visible: a hash-input change means one phantom update in every harness, which is why ADR D6 marked it one-way (`adr_harness_plugin_export.md:337`).
- What the comparable projects freeze is the **grammar and the discriminator** (`h1:`), not the algorithm's inputs. That matches stability.md today (grammar only), not ADR D11 (grammar plus hash input).
- Consequence: a policy that froze the grammar but let inputs evolve is coherent, provided evolution goes through a tag or counter and is release-noted. A policy that freezes the inputs makes even a *desired* renderer-fix bump a breaking change, which defeats the purpose.

### Recommendation
1. Choose **(b) input-addressed plus renderer-scheme counter**, not (c) and not (a) now. Hash = current member lines, plus one line `scheme\t<n>` emitted only when `n > 0`. Existing versions then stay byte-identical (no phantom update on introduction), and the first bump is the deliberate delivery of a renderer fix.
2. Reuse the `render_scheme` idea from `design_render_scheme_versioning.md` as the counter's home (per vendor family or one export-wide constant). Enforce bumps with a golden-output test that fails when a fixture's rendered bytes change without a counter change. (*inference:* no cited system relies on memory alone.)
3. Keep the grammar `<base>+<12-hex>`. Keep `+` (string-equality harnesses work; semver-aware ones are already an accepted limit).
4. Split the contract wording: **frozen** = grammar and the property "same declared inputs and same scheme give the same version"; **not frozen** = the concrete hash input list, changing only via a counter bump or an added input line, always release-noted. Align ADR D11 to stability.md (amend, since D11 is stricter than the shipped docs).
5. Treat `description`/`rename` edits as declared inputs (Nix logic): decide separately whether to add them to the hash input. That is a one-time phantom update and it reverses `test_s006` and the D6 accepted risk.
6. Revisit (a) only after byte determinism of directory outputs across Linux and Windows is proven; it removes the human step.
7. For phase 2's `[marketplace]` table: ship reader-first (release N parses it, N+1 writes it). Also add a reserved version-gate key now if pre-1.0 policy allows it. Document as "backward-compatible, not forward-compatible".

negative:
- No system found puts the **tool release number** in a user-visible update identifier. Cargo and Nix do it only for invisible caches.
- Could not fetch the Cargo fingerprint source, Bazel action-key docs, Nix store-path spec details, or Cargo's build-metadata section directly; those rows rely on the docs.rs fingerprint page, search summaries, and Nix pills.
- Whether Copilot, Codex or Cursor compare by string or semver is undocumented. Nothing here settles it.
- No evidence how Claude Code treats a manifest `version` that differs only by `+hash` across two re-exports beyond "string equality"; behaviour with prerelease-style `-hash` untested.

leads:
- Omit `version` entirely for git/`archive` marketplace sources and let Claude Code derive one (commit SHA or sha256). It would make renderer changes self-delivering for Claude, but "commit SHA of the installed directory" is ambiguous (tree vs commit) and per-harness behaviour is unknown.
- Verify `cargo-features` behaviour on old Cargo from primary Cargo tests or docs.
- Check Agent Plugins schema (`research_agent_plugins_spec_verify.md`) for any version comparison text.
- Measure directory-output determinism (mtime, mode bits, CRLF) across the WSL and Windows local test rigs before considering (a).
