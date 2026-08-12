# Handover: `insecure` as a per-registry config field

**Audience:** grim maintainers.
**Requested by:** grimoire-vscode (2026-08-12) — no extension blocker, the
env var works today; this is about the field living where the registry lives.
**Status:** request. No ADR, no grim branch. Contract below is a proposal,
not an accepted shape.

## TL;DR

Plain-HTTP opt-in is process-global env only
(`GRIM_INSECURE_REGISTRIES`, `src/oci/access/registry_client.rs:112`
`plain_http_hosts()`). Every other property of a registry — locator, alias,
default, include/exclude — is a `[[registries]]` field with a
`grim config registry set` surface. This one is not, and that asymmetry is
what the extension has to work around.

Ask: `insecure = true` on the `[[registries]]` entry, with the usual CLI and
`grim context` surfaces. Env var stays as the override.

## Why the env var is not enough

- **Wrong granularity.** `GRIM_INSECURE_REGISTRIES` is one flat host list for
  the process. A config with a plain-HTTP in-cluster registry AND a public
  HTTPS one cannot express "this entry, not that one" in the file that
  declares both — it has to be re-stated in every shell, CI job, systemd unit
  and launch config that runs grim.
- **Not portable with the project.** `grimoire.toml` is committed; the env var
  is not. A team member cloning a repo whose registry is HTTP gets
  `registry access failed` and has to be told out of band. Everything else
  needed to reach that registry is already in the file.
- **Invisible to any UI.** `grim context` reports `registries[]` — alias, url,
  kind, default, authenticated, include, exclude. A registry's transport is
  not in there, so neither the TUI nor the extension can show *why* a browse
  came back empty. Today the extension's only recourse is documenting the env
  var in a manual-testing guide.
- **Editors cannot set it.** The extension's settings panel edits registries
  through `grim config registry add/set`. A field with no config surface has
  no panel row; the user is sent to a VS Code env-var setting
  (`grimoire.extraEnv`) instead — a second, editor-specific door to a grim
  concept.

## Proposed contract

### Config

```toml
[[registries]]
alias = "local"
oci = "localhost:5050/grimoire"
insecure = true   # contact this registry over plain HTTP
```

`RegistryConfig` (`src/config/declaration.rs:240`) gains
`#[serde(default, skip_serializing_if = "is_false")] pub insecure: bool`.
Default `false`. Applies to the entry's own locator only — an `index` entry's
HTTP(S) base already carries its own scheme, so `insecure` is meaningful for
`oci` entries and (proposal) a parse error on `index` ones, matching how
`oci`/`index` mutual exclusion is already reported (exit 78).

### Resolution order

`plain_http_hosts()` becomes the union of, in order:

1. the current hardcoded loopback set (`localhost`, `127.0.0.1`, bare and
   `:5000`) — unchanged, still implicit;
2. every configured `[[registries]]` entry with `insecure = true`, by its
   locator host;
3. every `GRIM_INSECURE_REGISTRIES` entry — unchanged, still the override for
   hosts nothing declares (a `--registry` browse, `grim login` against an
   undeclared host, one-off `grim fetch`).

Union, not precedence: nothing here removes a host from the plain-HTTP set, so
there is no "config says secure, env says insecure" conflict to resolve. Note
this makes `plain_http_hosts()` config-dependent — it is called from
`RegistryClient::new` and `grim login` verification, both of which have a
resolved config in hand or can take the host set as a parameter.

### CLI

- `grim config registry add <alias> --oci <ref> --insecure`
- `grim config registry set <alias> --insecure` / `--no-insecure`
- `grim config set registry.<alias>.insecure true|false`, `grim config unset
  registry.<alias>.insecure` — the dotted-key route that already exists for
  every other field.

### JSON

`grim context` → `registries[]` gains `insecure: bool`. Always-present
(context is the always-present-null contract); the extension reads a missing
key as `false`, same tolerance pattern as `authenticated`. Additive, so it
does not break the frozen interface.

## What the extension does with it

- Settings panel: an "insecure (plain HTTP)" toggle on the registry row,
  written through `grim config registry set` like every other field. No VS
  Code setting, no `grimoire.extraEnv` instructions in the docs.
- A visible warning chip on registries that are HTTP, next to the existing
  auth chip — a transport downgrade the user opted into should be legible in
  the UI, not silent.
- Diagnosis of the empty-browse case: `grim search`'s `{items}` envelope has
  no warnings channel, so an unreachable HTTP registry is indistinguishable
  from an empty one (the failure is a tracing WARN on stderr). With `insecure`
  in `context`, the extension can at least say "this registry is HTTPS and the
  host looks local — did you mean `insecure = true`?" instead of nothing.

No sequencing dependency, and no compat shim wanted on either side: until the
field exists the extension keeps pointing users at `grimoire.extraEnv`
(`docs/manual-testing.md` §3b in grimoire-vscode), and switches over whenever
`registries[].insecure` shows up.

## Out of scope

- Per-registry TLS material (custom CA, client certs). Different problem,
  bigger contract, no demand from the extension.
- Any change to the implicit loopback set. Rig registries on `:5050`/`:5051`
  would still opt in explicitly — widening the implicit set to all of
  `localhost:*` is a security decision this request does not make.
