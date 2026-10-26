# Tech Strategy - Golden Paths

**SINGLE SOURCE OF TRUTH** for tech choices this project.

## Compliance

1. **Follow This File**: Use tech listed below
2. **No Deviations**: No alternatives unless told
3. **Latest Stable**: Latest stable version unless pinned

## Language Golden Paths

### Rust (Primary)

| Component | Choice |
|-----------|--------|
| Edition | Rust 2024 |
| Async | Tokio |
| Linker | Mold (dev) |

**Deviation: the `grim-hook` launcher** (`crates/grim-hook`, embedded in grim) is
synchronous `std`: no Tokio, clap, tracing, chrono or globset. It ships as a
small binary per target (512 KiB, 640 KiB on musl) that a client execs on
every tool call, so its dependency set is `serde`, `serde_json` and their
transitive crates, pinned by an allowlist test over
`cargo tree -p grim-hook -e normal --target all`. Everything else stays on Tokio. Rationale: `adr_hook_native_dispatcher.md` (C-N8, D1).

### Python (Acceptance Tests)

| Component | Choice |
|-----------|--------|
| Runtime | Python 3.13+ |
| Tooling | uv (Manager), Ruff (Linter) |
| Testing | pytest |

## Infrastructure

| Component | Choice |
|-----------|--------|
| Secrets | GitHub Secrets |

## CI/CD

| Component | Choice |
|-----------|--------|
| Platform | GitHub Actions |
| Auth | OIDC |
| Security | Trivy |