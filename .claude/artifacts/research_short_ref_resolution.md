# Research: Short-Reference Resolution (W1-B / D2)

- **Date:** 2026-07-26
- **Gate:** meta-plan_promotion_1_0 → W1-B research gate
- **Question:** *Does fixing short-ref expansion change resolution for any
  already-published reference?*
- **Answer:** **Yes — and the obvious fix is a breaking change, prohibited
  under Principle 9.** The resolution is narrower than the question implies:
  grim never required the kind segment, so the fix is a **documentation
  recommendation**, not an interface, a republish, or a migration (§5).
  Published paths stay exactly as they are.
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
| **B** | Publish first-party catalog at flat aliases too | Yes, zero code | First-party only — **dropped, see §5** |
| **C** | On `NotFound`, retry via index lookup (index already holds each package's exact `repo`) | Yes — only refs that 404 today change | Every kind-segmented publisher |
| **D** | Docs-only: stop implying a bare name resolves for first-party packages | Yes | Nothing; removes the false promise |

**A is off the table.** B, C, and D are all Principle-9 legal.

### Recommendation: **B + D**, with C recorded as the general fix

### Conclusion: **D only.** Nothing to build, publish, or migrate

Two framings in earlier drafts of this section were wrong in opposite
directions, and the correct line sits between them:

- **`grim` never required the kind segment.** `repository_prefix` and
  per-entry `repository` have always made the layout the publisher's choice;
  `{kind.subdir()}/{name}` is only the *default*. `ArtifactKind::subdir()` is
  defined at `src/oci/artifact_kind.rs:55` as *"the `$GRIM_HOME`/**install**
  subdirectory for this kind"* — an install-layout concept that also became
  the publish default. The kind itself always travels on the wire, read back
  by `kind_from_manifest` (`src/oci/annotations.rs:221`). So the segment is a
  **namespace partition, not a type tag**: it buys room for one name to exist
  as two kinds, and nothing else.
- **The already-published paths are still frozen.** They are live references
  that a `grimoire.lock` can pin. `ghcr.io/grimoire-rs/skills/grim-usage`
  stays, and resolves today (§3). Flipping the catalog flat and retiring the
  segmented names would break every consumer holding one — that is the
  breaking change, not the guidance.

**What is not frozen is the recommendation.** Changing which layout the docs
advise touches no interface at all. That is the entire fix:

- `docs/src/publishing.md` — new *"Do you need the kind segment?"* section:
  usually not; publish flat unless names collide across kinds; the layout also
  sets how short a reference users can type; choose before the first publish,
  because lockfiles pin paths.
- `docs/src/quickstart.md` — short refs expand verbatim, so the first-party
  short form is `skills/grim-usage`.

**B (flat aliases) is dropped.** It solves a problem the catalog does not
have: `skills/grim-usage` resolves, is documented, and works. Adding a second
permanent name for every package to save one path segment is not worth a
permanent dual identity — even though §6 shows it would be safe.

C stays recorded as the general fix for third-party kind-segmented
publishers, but it is ADR-sized (network-dependent resolution, ambiguity
rules, `GRIM_OFFLINE` degradation) and a flat *default recommendation*
largely prevents the situation from recurring.

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

## 7. Remaining questions

None blocking. The alias questions this section used to carry (registry
footprint, cascade across two repo names, `grim update` on an alias, flat
bundle member lists) died with option B — no alias is being published.

Open only if C is ever picked up: how a short id that matches packages in two
configured registries is disambiguated, and how index-assisted resolution
behaves under `GRIM_OFFLINE`.

## 8. ADR

- **D (what shipped): no ADR.** A documentation recommendation changes no
  interface, no resolution semantics, and no published path.
- **C: ADR required** — it adds a resolution fallback with network and
  ambiguity semantics.
