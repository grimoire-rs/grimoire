# Research: Reproducible zip writing in Rust for `grim export plugin --zip`

<!--
Technology Landscape Research
Filename: .agents/research/research_reproducible_zip_rust.md
Owner: Researcher (hex research phase)
Handoff to: Architect (/hex-architect), /hex-plan
Related Skills: hex-architect, hex-plan

Purpose: Persist tech landscape findings to inform ADRs, plans, design decisions.
Artifacts decay — check dates before trusting findings.
-->

## Metadata

**Date:** 2026-09-27
**Domain:** packaging
**Triggered by:** new `grim export plugin --zip` command needing byte-reproducible zip archives (same input → identical bytes, any machine/timezone)
**Expires:** 2027-03-27 (zip-rs/zip2 is on a 9.0 pre-release track; re-verify feature table before upgrading)

## Direct Answer

Use **`zip = "8.6"` (zip-rs/zip2), `default-features = false`, no compression feature enabled** — write every entry `CompressionMethod::Stored`. This sidesteps the entire deflate-determinism question, keeps the transitive dependency set to four always-on crates (`crc32fast`, `indexmap`, `memchr`, `typed-path` — all MIT/Apache-2.0/permissive, already deny.toml-clean), and needs no `time`/`chrono`/`jiff` feature because `zip::DateTime` is a crate-native type. Fix every entry's timestamp to `zip::DateTime::default()` (= 1980-01-01 00:00:00, the DOS epoch floor), set `unix_permissions(0o644)` (or `0o755` for the rare executable/dir case) explicitly, sort entries by their on-archive path before writing (mirrors the existing `tar` pattern in `src/skill/skill_package.rs:409`), and skip explicit directory entries (add files with their full relative path; zip readers synthesize directories from path components, same convention `tar::Builder` already follows in this codebase for `Regular`-only entries).

## Technology Landscape

### Established (proven, widely accepted)

| Tool/Pattern | Status | Notes |
|---|---|---|
| `zip` (zip-rs/zip2) | Mature, actively maintained | v8.6.0 released 2026-04-25 (stable); v9.0.0-pre3 exists 2026-08-11 (pre-release, not yet recommended). MSRV 1.88, "supports a minor Rust version stable ≥6 months." License MIT. 26.6M downloads/month, #1 in crates.io Compression category. Repo: `github.com/zip-rs/zip2` (fork continuing `zip-rs/zip2` after the original `zip-rs/zip` stalled — the crate name on crates.io is still `zip`). |
| Stored-only zip via a hand-rolled writer | Proven pattern, used by `repro-zipfile` (Python) | Viable, but reinvents CRC32/central-directory bookkeeping `zip` already gives for free — not worth it when `zip` crate exists and Stored needs zero extra deps. |

### Declining / not recommended for this use case

| Tool/Pattern | Status | Why not here |
|---|---|---|
| `async_zip` | Actively maintained (v0.0.19, 2026-08-22), MIT, tokio+futures IO traits | Still pre-1.0 (`0.0.x` — API not stabilized per its own versioning), and this command has no async I/O requirement (writes a local file synchronously); adding tokio-flavored zip writing for a sync CLI path is unneeded surface area. |
| `rawzip` | Actively maintained (v0.5.1, 2026-07-13), MIT, zero-dependency, zero-unsafe, explicitly low-level ("rejects batteries-included") | Deliberately minimal API — quickstart examples require hand-wiring `flate2` even for the common case, and no reproducibility guardrails (fixed timestamp helpers, permission builder) are documented. More assembly than `zip` for the same Stored-only outcome, no offsetting benefit. |
| Hand-rolled stored-only zip (no crate) | N/A | `zip` crate already gives Stored for free with zero extra transitive deps and default-features=false — reinventing central-directory/local-header-record byte layout is pure risk for a security-adjacent artifact (`quality-security.md` applies: this is a distribution artifact opened by a third-party app). |

## Design Patterns Worth Considering

- **Stored-only compression as a determinism shortcut** — Used by `repro-zipfile` (drivendataorg, PyPI) and implicit in reproducible-builds.org's archive guidance (`--no-extra`/`-X` to drop extra fields). Sidesteps the open question of whether Deflate bytes are stable across flate2 backend versions/builds (zip 8.6's default `deflate` feature now routes through `deflate-flate2-zlib-rs`, i.e. a zlib-rs backend, not the older `miniz_oxide`+`flate2` classic path — a moving target not worth pinning down for a plugin export whose payload is source text, not media). Plugin archives here are skills/rules/agent configs — small text — so the compression-ratio cost of Stored is negligible.
- **Fixed DOS-epoch timestamp, explicit unix perms, no extra fields** — the cross-language canonical recipe: Python's `repro-zipfile` sets `date_time=(1980,1,1,0,0,0)` and a fixed `external_attr`; the `fekir.info` writeup (independent verification) does the same plus `create_system = 3` (Unix) so `external_attr` permission bits are read correctly by Unix zip readers, and calls out that directories need `CRC=0` + the MS-DOS directory attribute bit, "unnecessary" but done by convention — grim's export skips directory entries entirely (matches the existing `tar` house pattern of files-only, path-implies-directory), so this doesn't apply here.
- **Sorted entry order** — reproducible-builds.org: "file ordering" is a top nondeterminism source; recommends `sort=name`/`LC_ALL=C sort`. `src/skill/skill_package.rs:409` already does exactly this (`files.sort_by(|a, b| a.0.cmp(&b.0))` on the on-wire packed name) before building the tar — the zip writer should sort the same way, over the same kind of key (archive-relative path string, not OS directory-iteration order).
- **`--no-extra` / suppress extra fields** — reproducible-builds.org explicitly recommends dropping zip "extra fields" (multiple redundant timestamp copies, platform metadata) because they vary per tool/OS. The `zip` crate does not add Unix UID/GID or Info-ZIP extended-timestamp (`0x5455`)/Unix (`0x7875`) extra fields unless the caller explicitly attaches an `ExtraField`; not calling any extra-field API is sufficient — there is no "enabled by default" extra field to turn off, unlike `zip -X` on the CLI tool which is stripping something the *shell* `zip` binary adds by default.

## Key Findings

1. **`zip` crate, default-features=false, transitive footprint is 4 crates.** Always-on (non-optional) deps regardless of feature selection: `crc32fast = "1.5"`, `indexmap = "2"`, `memchr = "2.7"`, `typed-path = "0.12"` — all permissive licenses, no `flate2`/`miniz_oxide`/`bzip2`/`zstd`/`time` pulled in when no compression feature is enabled. Confirmed via `zip-rs/zip2` `Cargo.toml` at tag v8.6.0. `Cargo.lock` in this repo currently has zero zip/flate2/miniz_oxide/crc32fast/bzip2/zstd entries — this is a clean net-new addition, not a version bump on something transitively present. [github.com/zip-rs/zip2 Cargo.toml](https://github.com/zip-rs/zip2/blob/v8.6.0/Cargo.toml)
2. **`CompressionMethod::Stored` needs no feature flag.** Docs state: "if neither `bzip2` nor `deflate` features are enabled, `CompressionMethod::Stored` becomes the default and files are written uncompressed." So `zip = { version = "8.6", default-features = false }` is sufficient — no `deflate`/`deflate-flate2` feature needed at all for this use case. [docs.rs/zip/8.6.0/zip/write](https://docs.rs/zip/8.6.0/zip/write/struct.FileOptions.html)
3. **`zip::DateTime` is crate-native, no `time` feature required.** `DateTime::default()` (== `DateTime::DEFAULT`) constructs exactly `1980-01-01 00:00:00`, the DOS-timestamp floor the zip format supports — matching the `tar::Header::set_mtime(0)` house pattern's intent (fixed, cross-machine-stable mtime) at the zip format's own epoch floor rather than Unix epoch 0 (zip's DOS date field cannot represent pre-1980 dates). The `time`/`chrono`/`jiff` features only add *conversion* methods to/from those crates' types — not needed if the writer only ever uses `DateTime::default()`. [docs.rs/zip/8.6.0/zip/struct.DateTime.html](https://docs.rs/zip/8.6.0/zip/struct.DateTime.html)
4. **`FileOptions` builder API is exactly what's needed, `const fn` all the way:** `compression_method(CompressionMethod::Stored)`, `unix_permissions(0o644u32)`, `last_modified_time(DateTime::default())`. No zip64 forcing needed — `large_file(bool)` exists but the 200 MB / 5,000-file plugin cap (per `.agents/research/research_claude_app_install_surfaces.md:44`) is nowhere near zip64 thresholds (4 GiB / 65,535 entries), so leave it `false`/default. [docs.rs/zip/8.6.0/zip/write/struct.FileOptions.html](https://docs.rs/zip/8.6.0/zip/write/struct.FileOptions.html)
5. **Entry ordering and no directory entries — matches the existing `tar` pattern exactly.** `src/skill/skill_package.rs:409-441` already sorts files by on-wire packed name and appends only `EntryType::Regular` entries (no directory nodes) with a stable header (`mode 0o644`, `mtime 0`, `uid/gid 0`). The zip writer should reuse the identical sort key and the identical files-only entry set — same list of `(entry_path, bytes)` pairs feeding a zip writer instead of (or alongside) the tar writer, so the two archive formats stay behaviorally identical for the same logical plugin content.
6. **reproducible-builds.org and independent practitioner writeups agree on the same four levers**: fixed timestamp (SOURCE_DATE_EPOCH-style, or a hardcoded floor value), stable file ordering (sorted, not directory-iteration order), fixed/explicit permissions in the archive's permission field, and stripping "extra fields" (`zip -X`/`--no-extra`) that otherwise carry redundant timestamps/platform metadata. `strip-nondeterminism` (Debian's reproducible-builds tool) implements exactly this for zip/jar post-processing, confirming these are the industry-standard levers, not something specific to Python. [reproducible-builds.org/docs/archives](https://reproducible-builds.org/docs/archives/), [github.com/esoule/strip-nondeterminism](https://github.com/esoule/strip-nondeterminism)
7. **Deflate-across-backends stability is a real open question, avoided entirely by choosing Stored.** zip 8.6's default `deflate` feature routes through `deflate-flate2-zlib-rs` (a newer, pure-Rust zlib-rs backend) rather than the classic `miniz_oxide` path — meaning even *within* the `zip` crate's own default features, the compression backend has changed version-over-version. Nothing found states deflate output is guaranteed byte-stable across flate2/zlib-rs/miniz_oxide versions; the safe answer for plugin exports (small text payloads, ratio doesn't matter) is to not use compression at all.
8. **No primary-source confirmation that Claude's own zip reader rejects Stored-only entries, missing directory entries, or requires/rejects zip64** — `.agents/research/research_claude_app_install_surfaces.md` and `research_plugin_support_matrix.md` document the plugin-upload size/file-count limits (200 MB / 5,000 files) and zip-root layout rule but say nothing about the zip reader's internal tolerance for compression method or directory entries. Web search surfaced only that Claude Code's `archive` marketplace source requires HTTPS delivery of the zip, nothing about the parser's format tolerance; no Node zip library was confirmed as Claude's implementation. This is **not independently verified** — treated as a residual risk, mitigated by the fact that Stored-only, no-zip64, no-directory-entries is the most conservative/widely-compatible zip shape possible (every general-purpose reader — Python `zipfile`, Java `java.util.zip`, the Node `yauzl`/`extract-zip` family, macOS/Windows built-in unzip — accepts it; it is the *strictest subset* of the format, not an edge case).
9. **`serde_json` struct field order needs no `preserve_order` feature.** `#[derive(Serialize)]` on a struct always serializes fields in their Rust declaration order — this is a property of `serde`'s derive macro (it emits `serialize_field` calls in source order) and is independent of `serde_json`'s `preserve_order` feature, which only affects `serde_json::Map`/`Value` (used for `HashMap`-shaped or dynamically-constructed JSON, not `#[derive(Serialize)]` structs). No new Cargo feature or dependency needed for stable `plugin.json` key order as long as it's emitted from a fixed-field-order struct, not a `HashMap`/`serde_json::Map` built at runtime.
10. **Trailing newline is a manual concern, not a serde_json default.** `serde_json::to_writer_pretty`/`to_string_pretty` do not append a trailing `\n`. Convention (matching most hand-authored JSON files, `jq`, and POSIX text-file expectations) is to append one `\n` byte after serialization — a one-line fix in the writer (`writer.write_all(b"\n")` after the JSON bytes), otherwise byte-for-byte identity depends on this being applied consistently across every run/machine (it will be, since it's not environment-dependent — noted for completeness, not because it's a determinism risk).

## Recommendation

**Crate:** `zip = "8.6"` (zip-rs/zip2), pinned to a concrete `8.6.x` (not the `9.0.0-pre*` track — pre-release, feature table may still shift).
**Cargo.toml:**
```toml
zip = { version = "8.6", default-features = false }
```
**Compression method:** `CompressionMethod::Stored` for every entry — no compression feature needed, avoids the deflate-backend-stability open question entirely, and payload is small config/text.
**Timestamp/permission policy:** every entry gets `last_modified_time(zip::DateTime::default())` (1980-01-01 00:00:00, the format's epoch floor) and `unix_permissions(0o644)` (0o755 only if the archive ever needs to mark an executable — not expected for plugin config). No directory entries (files-only, full relative paths — mirrors `src/skill/skill_package.rs`'s tar convention). No extra fields attached (don't call any `ExtraField` API — nothing is emitted by default). Entries sorted by archive-relative path before writing, reusing the same sort key `skill_package.rs:409` already applies for the tar path.

5-line writer sketch:
```rust
let mut zw = zip::ZipWriter::new(std::io::BufWriter::new(std::fs::File::create(out_path)?));
let opts = zip::write::FileOptions::<()>::default()
    .compression_method(zip::CompressionMethod::Stored)
    .unix_permissions(0o644)
    .last_modified_time(zip::DateTime::default());
for (entry_path, bytes) in entries /* pre-sorted by entry_path, same key as tar path */ {
    zw.start_file(entry_path, opts)?;
    zw.write_all(bytes)?;
}
zw.finish()?;
```

`plugin.json` bytes: build from a `#[derive(Serialize)]` struct with fields in the order they must appear on disk (no `preserve_order` feature needed), serialize with `serde_json::to_vec_pretty`/`to_writer_pretty`, then append a single trailing `\n` byte before handing the buffer to `start_file`/`write_all`.

## Sources

| Source | Type | Date | Relevance |
|--------|------|------|-----------|
| [crates.io/api/v1/crates/zip](https://crates.io/api/v1/crates/zip) | Registry API | fetched 2026-09-27 | Version history, license, download count |
| [lib.rs/crates/zip](https://lib.rs/crates/zip) | Aggregator | fetched 2026-09-27 | MSRV, default features, repo ownership (zip-rs/zip2) |
| [github.com/zip-rs/zip2 Cargo.toml @ v8.6.0](https://github.com/zip-rs/zip2/blob/v8.6.0/Cargo.toml) | Repo source | fetched 2026-09-27 | Feature table, always-on vs optional deps, MSRV/license fields |
| [docs.rs/zip/8.6.0/zip/struct.DateTime.html](https://docs.rs/zip/8.6.0/zip/struct.DateTime.html) | API docs | fetched 2026-09-27 | `DateTime::default()` = 1980-01-01 00:00:00, no `time` feature required |
| [docs.rs/zip/8.6.0/zip/write/struct.FileOptions.html](https://docs.rs/zip/8.6.0/zip/write/struct.FileOptions.html) | API docs | fetched 2026-09-27 | `compression_method`/`unix_permissions`/`last_modified_time`/`large_file` signatures; Stored needs no feature |
| [lib.rs/crates/async_zip](https://lib.rs/crates/async_zip) | Aggregator | fetched 2026-09-27 | Version 0.0.19, tokio+futures IO, pre-1.0 status |
| [lib.rs/crates/rawzip](https://lib.rs/crates/rawzip) | Aggregator | fetched 2026-09-27 | Version 0.5.1, zero-dep/zero-unsafe, low-level design philosophy |
| [reproducible-builds.org/docs/archives](https://reproducible-builds.org/docs/archives/) | Standards body docs | fetched 2026-09-27 | Canonical timestamp/ordering/permission/extra-field guidance |
| [github.com/esoule/strip-nondeterminism](https://github.com/esoule/strip-nondeterminism) | Repo (Debian reproducible-builds) | fetched 2026-09-27 | Confirms zip/jar extra-field stripping + timestamp normalization as established practice |
| [fekir.info/post/reproducible-zip-archives](https://fekir.info/post/reproducible-zip-archives/) | Practitioner blog | fetched 2026-09-27 | Independent verification of fixed date_time/external_attr/create_system recipe |
| [github.com/drivendataorg/repro-zipfile](https://github.com/drivendataorg/repro-zipfile) | Repo (Python) | fetched 2026-09-27 | Cross-language precedent: fixed 1980-01-01 date_time + fixed external_attr as the reproducibility recipe |
| `/home/mherwig/dev/grimoire-duo/deny.toml` | Repo file | read 2026-09-27 | License allowlist (MIT/Apache-2.0/BSD-3-Clause/ISC/Zlib/etc.) all zip-crate always-on deps satisfy |
| `/home/mherwig/dev/grimoire-duo/src/skill/skill_package.rs:400-441` | Repo file | read 2026-09-27 | Existing house reproducibility pattern (tar, sorted entries, fixed mtime/uid/gid, files-only) that the zip writer should mirror |
| `.agents/research/research_claude_app_install_surfaces.md` | Prior research (this repo) | read 2026-09-27 | Plugin zip layout/size limits (200 MB / 5,000 files); no zip-reader-internals detail found |
| `.agents/research/research_plugin_support_matrix.md` | Prior research (this repo) | read 2026-09-27 | Confirms zip is Anthropic-only transport; no reader-tolerance detail found |
| Web search: Claude Code plugin archive source / Node unzip library | Search (no single primary doc) | fetched 2026-09-27 | No primary-source confirmation of Claude's zip reader's Stored/zip64/directory-entry tolerance — flagged as unverified, mitigated by choosing the most conservative/compatible zip shape |
