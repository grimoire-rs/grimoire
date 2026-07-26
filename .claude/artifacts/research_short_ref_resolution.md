# Research: Short-Reference Resolution (W1-B / D2)

- **Date:** 2026-07-26
- **Gate:** meta-plan_promotion_1_0 → W1-B research gate
- **Question:** *Does fixing short-ref expansion change resolution for any
  already-published reference?*
- **Answer:** **Yes — and the obvious fix is a breaking change. It is
  prohibited under Principle 9.** The real finding is narrower: the kind
  segment was never required (§5), and identity is name-keyed so adding flat
  names is safe (§6).
- **Binary under test:** `target/release/grim` @ `f2bc57a`
- **Re-verify before citing:** 2027-01-26

---

## 1. What expansion does today

`Identifier::parse_with_default_registry` → `parse_internal`
(`src/oci/identifier.rs:338`) → `prepend_domain` (`:447`):

```rust
fn prepend_domain(name: &str, domain: &str) -> String {
    match name.split_once('/') {
        None => format!("{domain}/{name}"),
        Some((left, _)) => {
            if !(left.contains('.') || left.contains(':')) && left != "localhost" {
                format!("{domain}/{name}")
            } else { name.into() }
        }
    }
}
```

A plain concatenation. No kind segment is inserted, and none *can* be — see
§4. `has_explicit_registry` (`:330`) is what decides "this already names a
registry": first segment contains `.` or `:`, or equals `localhost`.

Behaviour is locked by four tests at `src/oci/identifier.rs:617-646`, e.g.
`default_registry_used_for_bare_name` asserts `code-review` + `ghcr.io` →
repository `code-review`. Any kind-segment insertion fails these.

## 2. Two incompatible layouts are already published

All 12 packages the public index serves (`grim search --format json`):

| Publisher | Layout | Repos |
|---|---|---|
| `grimoire-rs` | **kind-segmented** | `…/skills/{grim-usage,grim-authoring,ai-config-authoring}`, `…/bundles/grim-essentials`, `…/mcp/grim` |
| `michael-herwig/arcana` | **flat** | `…/arcana/{hex-core,hex-architect,hex-execute,hex-init,hex-plan,hex-review}` (skills) and `…/arcana/hex` (**bundle**) |

The arcana bundle `hex` and the arcana skills sit at *the same level*. There
is no kind segment anywhere in that namespace.

`catalog/README.md:68` documents kind-segmentation as the **grimoire-rs**
convention, driven by per-entry `repository` overrides in `publish.toml`. It
is a first-party choice, not a format requirement — which is exactly why the
two layouts coexist.

## 3. Measured resolution

```
$ grim describe grim-usage
  → ghcr.io/grimoire-rs/grim-usage:latest : tag not found        exit 79

$ grim describe skills/grim-usage
  → ghcr.io/grimoire-rs/skills/grim-usage:latest                 exit 0

$ GRIM_DEFAULT_REGISTRY=ghcr.io/michael-herwig/arcana grim describe hex-core
  → ghcr.io/michael-herwig/arcana/hex-core:latest  kind=skill    exit 0

$ GRIM_DEFAULT_REGISTRY=ghcr.io/michael-herwig/arcana grim describe hex
  → ghcr.io/michael-herwig/arcana/hex:latest       kind=bundle   exit 0
```

**Short refs are not broken.** They work correctly for flat layouts and fail
for kind-segmented ones. The first-party catalog is kind-segmented and the
built-in fallback registry is `ghcr.io/grimoire-rs`, so short refs fail for
precisely the packages the docs advertise.

`describe` and `add` share `parse_with_default_registry`, so both behave
identically. `search` is unaffected — it reads the index, which stores each
package's full `repo` string.

## 4. Why "insert the kind segment" is doubly wrong

**It breaks published packages.** With `GRIM_DEFAULT_REGISTRY` pointed at
arcana, `hex-core` resolves today (exit 0 above). Inserting `skills/` makes
it `…/arcana/skills/hex-core` → 404. That is a breaking change to a released
surface: **prohibited**, not merely risky.

**The kind is not known at expansion time.** Expansion happens during
`Identifier` parse; the kind is inferred later, from the fetched manifest
(`docs/src/quickstart.md:26-29`). Inserting a kind segment would mean
speculatively probing all five kinds — five registry round trips per short
ref, and ambiguous when two kinds share a name.

## 5. Options

| | Change | Additive? | Fixes |
|---|---|---|---|
| **A** | Insert kind segment in expansion | **No — breaking** | ✗ prohibited |
| **B** | Publish first-party catalog at flat aliases too | Yes, zero code | First-party only |
| **C** | On `NotFound`, retry via index lookup (index already holds each package's exact `repo`) | Yes — only refs that 404 today change | Every kind-segmented publisher |
| **D** | Docs-only: stop implying a bare name resolves for first-party packages | Yes | Nothing; removes the false promise |

**A is off the table.** B, C, and D are all Principle-9 legal.

### Recommendation: **B + D**, with C recorded as the general fix

> **Framing correction (2026-07-26, after review).** An earlier draft of this
> section presented B as a workaround — "publish aliases so the broken short
> ref starts working". That is backwards. **The kind segment was never
> required, and flat is the correct default.** `ArtifactKind::subdir()` is
> defined at `src/oci/artifact_kind.rs:55` as *"the `$GRIM_HOME`/**install**
> subdirectory for this kind"* — a local filesystem-layout concept that also
> became the registry-namespace default (`docs/src/publishing.md`, precedence
> rule 3). The kind always travels on the wire, read back by
> `kind_from_manifest` (`src/oci/annotations.rs:221`).
>
> So the segment is a **namespace partition, not a type tag**. It buys exactly
> one thing: room for one name to exist as two kinds. No first-party name
> collides across kinds, so the catalog pays a segment in every reference its
> users type and gets nothing back — while breaking its own short-ref promise.
>
> B is therefore not a workaround but the correct layout, applied additively.
> The segmented paths are frozen under Principle 9: going flat means **adding**
> names, never moving or retiring them.

B makes `grim add grim-usage` work with no code change and no resolution-
semantics change — the strongest possible Principle-9 position. D makes
`docs/src/quickstart.md:5-7` precise: it currently says grim "expands short
references against `ghcr.io/grimoire-rs`", which is literally true and
practically misleading.

C is the correct general answer and the only one that helps third-party
kind-segmented publishers, but it introduces a network-dependent resolution
path, needs an ambiguity rule for a name present in several registries, and
degrades under `GRIM_OFFLINE`. That is ADR-sized work for a problem only the
first-party catalog currently has — and one that a flat default largely
prevents from recurring.

## 6. Dual identity — tested, resolved

**The load-bearing risk in option B was: would a flat alias plus a
bundle-provided member read as one artifact or two?** Measured:

```
grim init
grim add ghcr.io/grimoire-rs/skills/grim-usage@sha256:17115400…   # digest-pinned, direct
grim add ghcr.io/grimoire-rs/bundles/grim-essentials:0            # provides ../skills/grim-usage:0
grim status --format json
  → 4 items: grim-essentials, grim-usage, ai-config-authoring, grim-authoring
    grim-usage appears ONCE, state=installed
```

Two textually different references — one digest-pinned, one floating and
deployment-relative — for the same binding name collapse into **one**
artifact. Identity is keyed on **(kind, binding name)**, not on the
repository path. The command surface agrees: `grim remove <kind> <name>`,
`grim uninstall <kind> <name>`. `docs/src/configuration.md:97` states the
resulting precedence: a direct declaration overrides a bundle member of the
same name.

A flat alias binds to the same last-path-segment name (`grim-usage`), so it
lands in the same slot. **No dual identity. Option B is safe on this axis.**

*Residual, untestable without publishing:* the true alias case differs from
the tested case only in the repository path, and the evidence above says the
path is not part of the key. Re-confirm on the first alias actually pushed.

## 7. Remaining questions for the sub-plan

1. Does an alias double the registry footprint, or does content-addressing
   dedupe it to extra tags?
2. Do the cascade rules (`adr_unified_publish_version_cascade.md`) apply
   cleanly to a second repo name for the same content?
3. Does `grim update` roll a flat-alias-installed artifact forward correctly?
4. Flat bundle members become `./grim-usage:0`; does the existing
   `grim-essentials` keep its `../skills/…` members, or does a flat bundle
   alias need its own member list?

## 8. ADR

- **B / D: no ADR.** B is catalog policy (`catalog/README.md` + `publish.toml`);
  D is docs. Neither changes resolution semantics.
- **C: ADR required** — it adds a resolution fallback with network and
  ambiguity semantics.
