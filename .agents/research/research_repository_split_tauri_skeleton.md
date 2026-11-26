# Research: Tauri 2 skeleton for grim-gui

## Metadata

- Date: 2026-11-26
- Expires: 2027-02-26
- Axis: technology (Tauri 2 desktop skeleton, repository split)
- Method: WebFetch of primary sources (crates.io API, npm registry, GitHub raw, v2.tauri.app, cargo book). Nothing installed, nothing built. Claims marked UNVERIFIED were not compile-tested.

## Direct Answer

- Pin `tauri = "2.12.5"`, `tauri-build = "2.7.4"` (both published 2026-10-21, over 5 weeks old). Newer 2.12.6/2.12.7 (2026-11-16/18) fail the two-week rule. Pin `@tauri-apps/cli` and `@tauri-apps/api` to `2.12.5`. The npm publish dates were not read, so confirm they are older than 2026-11-12 with `npm view`.
- Binary name: Cargo package name `grim-gui` gives executable `grim-gui`. Set `mainBinaryName` only if the package is named otherwise.
- `unsafe`: `#[tauri::command]`, `generate_handler!` and `generate_context!` emit no `unsafe` (read from source). `mobile_entry_point` emits `#[unsafe(no_mangle)]`, but the template gates it with `cfg_attr(mobile, ...)`, so desktop never expands it. One residual risk: on macOS dev-profile builds `generate_context!` embeds an Info.plist through `embed_plist`, whose expansion uses `unsafe`, `#[no_mangle]`, `#[link_section]`. External `macro_rules` expansions normally suppress this lint, so `forbid` probably holds. UNVERIFIED until the macOS CI job compiles it.
- Docker: Ubuntu 24.04 image with the apt list in section 4, rustup 1.95.0, Node 24, repo bind-mounted. Build with `npm ci && npm run build && cargo build --release -p grim-gui` or `npx tauri build --no-bundle`.
- CI: `tauri-apps/tauri-action` is not needed for an unsigned, unreleased build. Use `npm ci && npx tauri build --no-bundle` per OS. If the action is wanted, `action-v1.0.0` = `1deb371b0cd8bd54025b384f1cd735e725c4060f`.

## 1. Versions and template layout

crates.io (created_at): `tauri` 2.12.7 (2026-11-18), 2.12.6 (11-16), 2.12.5 (10-21), 2.12.4 (10-14); `tauri-build` 2.7.6 (11-18), 2.7.5 (11-16), 2.7.4 (10-21). 3.0.0-alpha.x also exists; ignore. npm `latest`: `@tauri-apps/cli` 2.12.6, `@tauri-apps/api` 2.12.5 (so api 2.12.5 is the lowest common line).

create-tauri-app (`npm create tauri-app@latest`, choose npm, Vanilla, TypeScript). Template source, `dev` branch:
- Frontend (`templates/template-vanilla-ts`): `index.html`, `package.json`, `tsconfig.json`, `vite.config.ts`, `src/main.ts`, `src/styles.css`, `src/assets/typescript.svg`, `.vscode/extensions.json`, `.gitignore`.
- Rust (`templates/_base_/src-tauri`): `Cargo.toml`, `build.rs`, `tauri.conf.json`, `capabilities/default.json`, `icons/` (png, ico, icns, Square*Logo.png), `src/main.rs`, `src/lib.rs`, `.gitignore`.
- `package.json` scripts: `dev: vite`, `build: tsc && vite build`, `preview`, `tauri: tauri`. devDependencies at template time: `vite ^8.0.16`, `typescript ~6.0.3`, `@tauri-apps/cli ^2`; dependencies `@tauri-apps/api ^2`, plus `@tauri-apps/plugin-opener ^2` (drop it: it is a plugin, not wanted here).
- `vite.config.ts`: `clearScreen: false`, port 1420 `strictPort`, HMR host via `TAURI_DEV_HOST`, ignores `**/src-tauri/**`.
- `src-tauri/Cargo.toml`: edition 2024, `[lib]` with `crate-type = ["staticlib","cdylib","rlib"]`, deps `tauri`, `tauri-build` (build-dep), `tauri-plugin-opener`, `serde`, `serde_json`. Release profile: `codegen-units=1, lto, opt-level=3, panic="abort", strip`.
- `main.rs`: `#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]` then `<lib>::run()`. `lib.rs` holds `run()` plus `#[cfg_attr(mobile, tauri::mobile_entry_point)]`.
- Template `tauri.conf.json`: `build.frontendDist` `../dist`, `devUrl http://localhost:1420`, `beforeDevCommand "npm run dev"`, `beforeBuildCommand "npm run build"`, `security.csp: null`, `bundle.active: true`.
- Skeleton trims: remove opener plugin (Cargo, `lib.rs`, capability, npm), remove mobile attribute and `staticlib/cdylib` crate types (desktop only; `rlib` suffices). The template's `run()` ends with `.expect(...)`: it trips the `expect_used` warn, so return/handle the error in `main`.

## 2. Binary name

Docs for `mainBinaryName` say it renames the output binary, omit the extension, and "recommends changing the Cargo package or binary name instead when possible". `productName` names bundles and platform metadata only (macOS `.app`/`.dmg`, Linux deb/rpm kebab-case, Windows installers), not the plain executable. Recommendation: `[package] name = "grim-gui"` (binary default = package name), `productName: "grim-gui"`, lib name `grim_gui_lib` (template's own `_lib` suffix avoids a Windows lib/bin name clash). Keep `identifier` unique (`com.tauri.dev` is rejected by `tauri build`). Check `target/release/grim-gui[.exe]` after build.

## 3. unsafe_code = "forbid"

Read from `tauri-apps/tauri` `dev` branch:
- `tauri-macros/src/command/wrapper.rs` (`#[tauri::command]`, `generate_handler!`): generated code has only `#[allow(unused...)]`, `#[allow(unreachable_code, clippy::...)]`. No `unsafe`.
- `tauri-codegen/src/context.rs` (`generate_context!`): only `#[allow(unused_mut, clippy::let_and_return)]`. No `unsafe` in the quote blocks. It calls `tauri::embed_plist::embed_info_plist!` only when compiled on a macOS host, target macOS, `dev` set, not running tests.
- `tauri-macros/src/mobile.rs`: emits `#[unsafe(no_mangle)]` on `start_app` for non-Android. `forbid` would reject it, and `forbid` cannot be lowered by an inner `allow`. Template avoids it via `#[cfg_attr(mobile, tauri::mobile_entry_point)]`; desktop-only skeleton should omit the attribute entirely.
- `embed_plist` (docs.rs source): expansion has `#[link_section = ...]`, `#[no_mangle]`, and an `unsafe { Transmute ... }` const. No `allow(unsafe_code)`.
- No upstream issue about `forbid(unsafe_code)` was found (see Negative results). Source reading is the only evidence.

Narrowest fallback if the macOS job fails: `src-tauri/Cargo.toml` uses its own `[lints.rust] unsafe_code = "deny"` (cargo does not allow `workspace = true` plus per-key override of the same table), and `lib.rs` puts `#[allow(unsafe_code)]` on the single `pub fn run()` that contains `tauri::generate_context!()`. The `grim-sdk` and other crates keep `forbid`. Add a one-line `// why:` comment. Workspace lint stays `forbid` for everything else.

## 4. Ubuntu 24.04 packages and Docker

Prerequisites page (v2, Debian/Ubuntu): `libwebkit2gtk-4.1-dev build-essential curl wget file libxdo-dev libssl-dev libayatana-appindicator3-dev librsvg2-dev`. Node only needed for a JS front end (LTS). Rust through rustup.

Dockerfile shape (UNVERIFIED, not built):
```
FROM ubuntu:24.04
ENV DEBIAN_FRONTEND=noninteractive
RUN apt-get update && apt-get install -y --no-install-recommends \
    libwebkit2gtk-4.1-dev build-essential curl wget file libxdo-dev libssl-dev \
    libayatana-appindicator3-dev librsvg2-dev ca-certificates git pkg-config \
    xvfb xauth dbus-x11 libgl1-mesa-dri && rm -rf /var/lib/apt/lists/*
RUN curl -fsSL https://deb.nodesource.com/setup_24.x | bash - && apt-get install -y nodejs
ENV RUSTUP_HOME=/opt/rustup CARGO_HOME=/opt/cargo PATH=/opt/cargo/bin:$PATH
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain 1.95.0
WORKDIR /work
```
Run: `docker run --rm --user $(id -u):$(id -g) -e HOME=/tmp -e CARGO_TARGET_DIR=/work/src-tauri/target -v "$PWD:/work" -v grim-gui-cargo:/opt/cargo/registry IMAGE sh -c 'npm ci && npx tauri build --no-bundle'` (a named volume for the registry; CARGO_HOME is in the image, so non-root needs it writable or `CARGO_HOME=/tmp/cargo`). Use a rust-toolchain.toml pinning 1.95.0 so host and CI agree. A glibc build on 24.04 links `libwebkit2gtk-4.1.so.0`, so it runs on 24.04 only (fine for local test).

Headless smoke test: yes, `xvfb-run -a ./target/release/grim-gui` in the container; the webview needs a display. Packages: `xvfb`, `xauth`, plus runtime libs already pulled by the `-dev` package (`libwebkit2gtk-4.1-0`); `dbus-x11` and `libgl1-mesa-dri` for software GL. UNVERIFIED: WebKit often needs `WEBKIT_DISABLE_COMPOSITING_MODE=1` or `WEBKIT_DISABLE_DMABUF_RENDERER=1` in containers, and the bubblewrap sandbox may need `--privileged` or unconfined seccomp. The WebDriver CI page confirms xvfb-run use for tests on Ubuntu (`webkit2gtk-driver xvfb`).

## 5. Capabilities and CSP

Doc fact: commands registered via `invoke_handler` are by default "allowed to be used by all the windows and webviews of the app"; no capability is needed. To restrict to one named command, in `build.rs`:
`tauri_build::try_build(tauri_build::Attributes::new().app_manifest(tauri_build::AppManifest::new().commands(&["grim_version"])))`, then the capability grants `allow-grim-version`. (The `AppManifest::commands` API is documented; the exact `Attributes` chain is from memory, UNVERIFIED.)
`capabilities/default.json`:
```
{ "$schema": "../gen/schemas/desktop-schema.json", "identifier": "default",
  "windows": ["main"], "permissions": ["allow-grim-version"] }
```
Grant no `core:default` and no plugins: no shell, no fs. `core:default` is only needed for event/window APIs; the invoke itself does not need it (UNVERIFIED on 2.12.5; test that `invoke` works with only the custom permission).
CSP (docs: `security.csp` is a string or directive map; Tauri injects nonces/hashes for its own assets unless `dangerousDisableAssetCspModification`): `"csp": "default-src 'self'; script-src 'self'; style-src 'self'; img-src 'self' data:; connect-src ipc: http://ipc.localhost"`. Dev server (HMR, `ws://localhost:1420`) is a dev-only concern: add `devCsp` or keep dev-only relaxation; `devCsp` semantics were not confirmed from the page. Avoid inline `<script>` and `<style>` in `index.html`.

## 6. Testing

- Unit-test the logic as a plain Rust fn: keep the command a thin wrapper over a `grim_sdk` call (`fn grim_version() -> Result<Value, String>`); test the fn and the sdk with a fake `grim` binary on PATH.
- `tauri::test` (feature `test`, `mock_builder`, `mock_context(noop_assets())`, `mock_app`, `get_ipc_response`, `assert_ipc_response`) exists in `crates/tauri/src/test/mod.rs`; its module doc says "This module is unstable". Usable but not a contract; the mocking docs page does not cover it. Worth one smoke test only.
- Front end: `vitest` with `happy-dom` or `jsdom`; `@tauri-apps/api/mocks` `mockIPC((cmd,args)=>...)` + `clearMocks()` after each test. jsdom lacks WebCrypto, so polyfill `crypto.getRandomValues` (docs) or use happy-dom (UNVERIFIED if needed).
- WebDriver `tauri-driver`: Windows and Linux only ("macOS has no WKWebView driver"). Needs `cargo install tauri-driver --locked`, `webkit2gtk-driver`, `xvfb` on Linux. Not worth it for a one-page skeleton; revisit when there are flows.

## 7. CI

- `tauri-apps/tauri-action`: releases `action-v1.0.0` (2026-06-29, commit `1deb371b0cd8bd54025b384f1cd735e725c4060f`), `action-v0.6.2` (2026-03-14, `84b9d35b5fc46c1e45415bdb6144030364f7ebc5`). Docs example uses `@v1`. It builds, bundles, and creates releases/uploads; for an unsigned smoke build it adds nothing. Skip it until releases.
- Plain steps per matrix (`ubuntu-24.04`, `macos-latest`, `windows-latest`): `actions/setup-node` (Node 24, npm cache), toolchain from `rust-toolchain.toml`, `Swatinem/rust-cache@v2` with `workspaces: ". -> target"` (docs show `./src-tauri -> target` for the default layout; here the root workspace owns `target/`), `npm ci`, `npx tauri build --no-bundle`. `--no-bundle` skips installers (deb/AppImage need extra tools like `patchelf`), so Linux needs only the section 4 list. Docs example installs on Linux: `libwebkit2gtk-4.1-dev libappindicator3-dev librsvg2-dev patchelf xdg-utils` (on ubuntu-22.04; use the prerequisites list on 24.04).
- Pin every action by commit SHA with a version comment. SHAs not looked up here except tauri-action above; docs example uses `actions/checkout@v7`, `actions/setup-node@v6`.
- Upload `target/release/grim-gui*` with `actions/upload-artifact` if needed.
- Windows: no extra system deps; WebView2 is preinstalled on runners (UNVERIFIED).

## 8. Dependabot and gitignore

- `.github/dependabot.yml`: `package-ecosystem: npm, directory: "/"`; `cargo, directory: "/"` (the root workspace holds `Cargo.lock`; a `src-tauri` entry is needed only if there is no root workspace); `github-actions, directory: "/"`. Group `tauri` + `tauri-build` + `@tauri-apps/*` so versions move together (tauri's own npm/crate versions should stay in lockstep minor).
- `.gitignore`: `node_modules`, `dist`, `target` (root workspace) or `src-tauri/target`, `src-tauri/gen`. The template `src-tauri/_gitignore` is `/target/` and `/gen/schemas`. `gen/schemas` is generated by the build and only gives IDE autocompletion for capabilities: do not commit. Non-schema `gen/` content only exists for mobile (`gen/android`, `gen/apple`), not relevant here. Commit `package-lock.json` and `Cargo.lock` (binary app).

## 9. Workspace and git dependency

Cargo book: `[patch."<git url>"]` is supported with `path` entries; the patched version must satisfy the dependency requirement; `[patch]` is read only from the workspace root manifest, so it goes in grim-gui's root `Cargo.toml`. After adding, `Cargo.lock` re-resolves; if stale use `cargo update -p grim-sdk`. The URL key must match the dependency's `git` URL string exactly (including no trailing slash/`.git` mismatch; UNVERIFIED how cargo canonicalizes; use the identical string). A path patch should live in `.cargo/config.toml` (`--config` form) rather than the committed manifest if the local override must not ship; CI then uses the pinned `rev`.
Submodules: cargo checks out git dependencies and updates their submodules (forum threads; cargo issue #7987 "recursively clone submodules for git dependencies", #4247 opt-out stalled). The grimoire repo's two `external/` submodules would be cloned on every fresh fetch. Avoidance options: (a) cargo patch with a `path` does not fetch the git source once the lock is resolved, but a clean CI checkout still resolves the git source first, so it still clones; (b) publish `grim-sdk` as its own small repo or crate; (c) vendor with `cargo vendor` in CI; (d) set `net.git-fetch-with-cli = true` and a global `submodule.recurse=false`, UNVERIFIED that it avoids the checkout. Recommend (b) or accept the clone cost.

## Negative results

- No GitHub issue or discussion for `forbid(unsafe_code)` with `generate_context!`/`tauri::command` was found (two searches).
- The v2 project-structure page does not mention `gen/` or recommended ignores; the template `_gitignore` was the only source.
- The mocking docs page does not document the Rust `tauri::test` module; only the source doc says it is unstable.
- Publish dates for `@tauri-apps/cli` and `@tauri-apps/api` npm versions were not retrievable (response truncated); `cli` latest 2.12.6, `api` latest 2.12.5.
- `devCsp` and `freezePrototype` semantics were not visible on the config reference excerpt.
- Cargo has no documented switch to skip submodules of a git dependency.

## Sources

- https://crates.io/api/v1/crates/tauri , https://crates.io/api/v1/crates/tauri-build
- https://registry.npmjs.org/@tauri-apps/cli , https://registry.npmjs.org/@tauri-apps/api
- https://v2.tauri.app/start/prerequisites/ , /start/create-project/ , /start/project-structure/
- https://v2.tauri.app/reference/config/ , /security/capabilities/ , /develop/calling-rust/
- https://v2.tauri.app/develop/tests/webdriver/ , /develop/tests/webdriver/ci/ , /develop/tests/mocking/
- https://v2.tauri.app/distribute/pipelines/github/
- https://github.com/tauri-apps/create-tauri-app/tree/dev/templates (raw files read)
- https://github.com/tauri-apps/tauri/blob/dev/crates/tauri-macros/src/mobile.rs , .../command/wrapper.rs , crates/tauri-codegen/src/context.rs , crates/tauri/src/lib.rs , crates/tauri/src/test/mod.rs
- https://docs.rs/embed_plist/latest/src/embed_plist/lib.rs.html
- https://api.github.com/repos/tauri-apps/tauri-action/releases and git/ref/tags/action-v1.0.0, action-v0.6.2
- https://doc.rust-lang.org/cargo/reference/overriding-dependencies.html
- https://github.com/rust-lang/cargo/issues/7987 , https://users.rust-lang.org/t/avoid-cargo-cloning-git-submodules/104752
