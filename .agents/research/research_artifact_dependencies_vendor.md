# Research: Artifact dependencies — vendor semantics detail

## Metadata

**Date:** 2026-11-01
**Domain:** packaging
**Triggered by:** `.agents/discussions/artifact-dependencies.md` — competitive/vendor lane
**Expires:** 2027-05-01

## Findings

**Claude Code plugin dependencies** ([deps](https://code.claude.com/docs/en/plugin-dependencies), [troubleshooting](https://code.claude.com/docs/en/plugins/troubleshooting), [CLI](https://code.claude.com/docs/en/plugins/cli-reference))
- **No satisfying tag.**
  - A plugin with its own repo source fails with `has no git tag satisfying`.
  - A plugin referenced by a relative path installs the marketplace's current copy, re-checks the range at load, and disables an out-of-range dependent.
  - npm, archive and command sources check the range at load only.
- **Removal.**
  - `disable` is refused and names the dependents.
  - `uninstall` leaves auto-installed dependencies until `claude plugin prune`, which never removes user-installed plugins.
- **Conflicts.**
  - Constraints are intersected, and the highest satisfying tag wins.
  - A disjoint range fails the install with `has conflicting version requirements` and leaves the existing state unchanged.
  - Auto-update keeps the conflicted dependency and reports it in the Errors tab.
  - Before v2.1.113, a conflicting install succeeded silently ([#50145](https://github.com/anthropics/claude-code/issues/50145)).
- **Cross-marketplace.** Unless allowlisted, the plugin installs without the dependency and then fails to load.

**APM** ([MCP primitive](https://microsoft.github.io/apm/producer/author-primitives/mcp-as-primitive/), [uninstall](https://microsoft.github.io/apm/reference/cli/uninstall/))
- **Self-defined MCP servers.**
  - A self-defined server (`registry: false`) is trusted only from a direct dependency.
  - At depth greater than 1 it is skipped with a warning unless `--trust-transitive-mcp` is passed.
  - Registry-resolved servers pass, because they are "vetted by the registry".
  - The docs give no threat rationale.
- **Uninstall.** It prunes transitive dependencies nothing needs and silently keeps shared diamond dependencies.
- **Diamond policy.** Undocumented.

**Helm** ([HIP #441](https://github.com/helm/community/pull/441), [helm#13077](https://github.com/helm/helm/issues/13077))
- Dependencies are specified by version, which maps to a mutable tag.
- Digest locking is an unmerged draft.
- The same subchart at two versions is unsupported. The workaround is `alias`.

**Cross-kind dependencies.** APM keeps separate `apm` and `mcp` dependency lists. skillpm keeps `mcpServers` outside npm resolution. No tool has a Debian-style recommends/suggests tier.

negative:
- No documented Claude Code behavior for uninstalling a dependency a dependent still needs.
- No APM diamond policy.
- No shipped Helm digest lock.

leads:
- APM resolver source.
- Helm `pkg/resolver`.
- `uaaps.github.io/uaaps_docs/spec/02-manifest`.
