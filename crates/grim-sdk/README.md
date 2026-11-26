<!-- doc_type: readme -->
# grim-sdk

Drives the `grim` binary from Rust: finds it, runs it, parses its JSON
output, checks its version, maps its exit codes.

- **Unstable** and **unpublished** (`publish = false`, own version line).
- Not a library API of grim. It invokes the binary only and links nothing
  from the `grimoire` crate.

## Finding the binary

`bin_from_env()` returns an absolute `GRIM_BIN` when set, else `grim` on
`PATH`.

```rust
let grim = grim_sdk::GrimDriver { bin: grim_sdk::bin_from_env() };
let version = grim.version()?;
println!("grim {version}");
```
