# Client Compatibility

<!-- TODO(vendor-wave): intro prose — what this page is and how to read it. -->

grim installs each artifact kind into the AI client(s) you target. Support
varies by client and kind. This matrix is the enforced source of truth:
a Rust table-parity test (`src/install/client_target.rs`) fails the build if
it drifts from the `Vendor` implementations.

Legend: `✓` supported (native or transform), `◐` supported with a documented
limitation (footnote), `✗` declined (see [Known gaps](#known-gaps)).

## Support matrix {#matrix}

<!-- TODO(vendor-wave): fill cells per the Vendor impls / adr_vendor_wave_expansion
     mapping table. Rows MUST match ClientTarget::ALL order and set exactly
     (the table-parity test enforces row-set equality). -->

| Client | Skill | Rule | Agent | MCP |
|--------|-------|------|-------|-----|
| Claude | TODO | TODO | TODO | TODO |
| OpenCode | TODO | TODO | TODO | TODO |
| Copilot | TODO | TODO | TODO | TODO |
| Codex | TODO | TODO | TODO | TODO |
| Cursor | TODO | TODO | TODO | TODO |
| Kiro | TODO | TODO | TODO | TODO |
| Junie | TODO | TODO | TODO | TODO |
| Gemini | TODO | TODO | TODO | TODO |
| Zed | TODO | TODO | TODO | TODO |
| Amp | TODO | TODO | TODO | TODO |

Bundles decompose into their member kinds and are not a column.

## Known gaps {#known-gaps}

<!-- TODO(vendor-wave): promote the user-relevant vendor-capability-watchlist.md
     rows here — rationale + upstream tracking pointer per decline. -->

## The `compatibility:` frontmatter field {#compatibility-disclaimer}

<!-- TODO(vendor-wave): explicit statement that the `compatibility:` frontmatter
     field is a free-text editor/runtime hint with zero effect on grim's
     per-vendor rendering, and that this matrix is the enforced truth. -->
