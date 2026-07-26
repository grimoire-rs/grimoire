# Research: Short-Reference Resolution (W1-B / D2)

- **Date:** 2026-07-26
- **Gate:** meta-plan_promotion_1_0 → W1-B research gate
- **Question:** *Does fixing short-ref expansion change resolution for any
  already-published reference?*
- **Answer:** **Yes — and the obvious fix is a breaking change. It is
  prohibited under Principle 9.**
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

B makes `grim add grim-usage` work with no code change and no resolution-
semantics change — the strongest possible Principle-9 position. D makes
`docs/src/quickstart.md:5-7` precise: it currently says grim "expands short
references against `ghcr.io/grimoire-rs`", which is literally true and
practically misleading.

C is the correct general answer and the only one that helps third-party
kind-segmented publishers, but it introduces a network-dependent resolution
path, needs an ambiguity rule for a name present in several registries, and
degrades under `GRIM_OFFLINE`. That is ADR-sized work for a problem only the
first-party catalog currently has.

## 6. Open questions for the sub-plan

1. **Dual identity.** If `grim-usage` is installed via a flat alias while
   `grim-essentials` provides it via `../skills/grim-usage:0`, do `status`
   and the effective-set logic (`adr_effective_set_mutations.md`) treat them
   as one artifact or two? **This is the load-bearing risk in option B** and
   must be tested before publishing any alias.
2. Does an alias double the registry footprint, or does content-addressing
   dedupe it to extra tags?
3. Do the cascade rules (`adr_unified_publish_version_cascade.md`) apply
   cleanly to a second repo name for the same content?
4. Does `grim update` roll a flat-alias-installed artifact forward correctly?

## 7. ADR

- **B / D: no ADR.** B is catalog policy (`catalog/README.md` + `publish.toml`);
  D is docs. Neither changes resolution semantics.
- **C: ADR required** — it adds a resolution fallback with network and
  ambiguity semantics.
