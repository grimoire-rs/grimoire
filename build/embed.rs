// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! Pure helpers for the nested `grim-hook` build in `build.rs` (C-N2).
//!
//! Included by `build.rs` and by grim's unit tests through `#[path]`, so the
//! decisions that the script makes blind (no test harness runs a build script)
//! stay testable.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// Variables cargo sets for grim's own build script; none describes `grim-hook`.
const DENY_PREFIXES: &[&str] = &[
    "CARGO_PKG_",
    "CARGO_MANIFEST_",
    "CARGO_CFG_",
    "CARGO_FEATURE_",
    // Coverage instrumentation must not reach the size-budgeted launcher.
    "CARGO_LLVM_COV",
];

const DENY_EXACT: &[&str] = &[
    "CARGO_PRIMARY_PACKAGE",
    "OUT_DIR",
    "TARGET",
    "HOST",
    "PROFILE",
    "OPT_LEVEL",
    "DEBUG",
    "NUM_JOBS",
    "RUSTC",
    "RUSTDOC",
    "RUSTC_LINKER",
    "CARGO_ENCODED_RUSTFLAGS",
    // clippy-driver, sccache: wrapping the nested build would lint or cache the
    // launcher. The workspace wrapper has its own rule, `is_denied_env`.
    "RUSTC_WRAPPER",
    "CARGO_BUILD_RUSTC_WRAPPER",
    "LLVM_PROFILE_FILE",
];

const ENCODED_FLAG_SEP: char = '\x1f';

/// Env vars naming a wrapper for workspace members, which includes `grim-hook`.
pub const WORKSPACE_WRAPPER_VARS: &[&str] = &["RUSTC_WORKSPACE_WRAPPER", "CARGO_BUILD_RUSTC_WORKSPACE_WRAPPER"];

/// Whether `name=value` must be kept out of the nested build's environment.
///
/// `cargo auditable build` (how dist builds releases) injects itself as the
/// workspace wrapper, and the launcher's `.dep-v0` section exists only if the
/// nested build keeps it. Any other wrapper is dropped. The wrapper panics
/// without `CARGO_AUDITABLE_ORIG_ARGS`, so that variable passes the filter; the
/// wrapper reads it only to run `cargo metadata`, where the outer flags still
/// resolve `grim-hook` because the nested build runs at the same workspace root.
pub fn is_denied_env(name: &str, value: &str) -> bool {
    let name = name.to_ascii_uppercase();
    if WORKSPACE_WRAPPER_VARS.contains(&name.as_str()) {
        return !matches!(
            Path::new(value).file_name().and_then(|n| n.to_str()),
            Some("cargo-auditable" | "cargo-auditable.exe")
        );
    }
    DENY_EXACT.contains(&name.as_str()) || DENY_PREFIXES.iter().any(|p| name.starts_with(p))
}

/// Cargo profile for the nested build, from the outer `PROFILE` (`release` or `debug`).
pub fn nested_profile(outer_profile: &str) -> &'static str {
    if outer_profile == "release" { "hook" } else { "hook-dev" }
}

/// Directory the nested build owns. Its own dir (not the outer one) so it
/// never contends for the outer build's target-dir lock.
///
/// A relative `CARGO_TARGET_DIR` resolves against the package root, matching
/// how the xwin tasks set it. Unset, the outer dir is recovered from
/// `OUT_DIR` = `<T>/[<triple>/]<profile>/build/<pkg>-<hash>/out`.
pub fn embed_target_dir(
    cargo_target_dir: Option<&str>,
    manifest_dir: &Path,
    out_dir: &Path,
    target: &str,
) -> Option<PathBuf> {
    let outer = match cargo_target_dir.filter(|d| !d.is_empty()) {
        Some(dir) => manifest_dir.join(dir),
        None => {
            let above_profile = out_dir.ancestors().nth(4)?;
            let outer = if above_profile.file_name().is_some_and(|n| n == target) {
                above_profile.parent()?
            } else {
                above_profile
            };
            outer.to_path_buf()
        }
    };
    Some(outer.join("grim-hook-embed"))
}

/// The arguments after `cargo`.
pub fn nested_args(profile: &str, target: &str, target_dir: &Path, offline: bool) -> Vec<String> {
    let mut args: Vec<String> = ["build", "-p", "grim-hook", "--profile", profile]
        .map(String::from)
        .into();
    args.extend(["--target".into(), target.into(), "--locked".into()]);
    args.extend(["--target-dir".into(), target_dir.display().to_string()]);
    if offline {
        args.push("--offline".into());
    }
    args
}

pub fn is_truthy(value: &str) -> bool {
    matches!(value.to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on")
}

fn is_instrumentation(codegen_option: &str) -> bool {
    codegen_option.starts_with("instrument-coverage") || codegen_option.starts_with("profile-generate")
}

/// Splits `CARGO_ENCODED_RUSTFLAGS` and drops coverage / PGO-generate flags,
/// in both the `-Cx` and the `-C x` spelling.
pub fn filter_rustflags(encoded: &str) -> Vec<String> {
    let mut flags = encoded.split(ENCODED_FLAG_SEP).filter(|f| !f.is_empty());
    let mut kept = Vec::new();
    while let Some(flag) = flags.next() {
        if flag == "-C" {
            match flags.next() {
                Some(opt) if is_instrumentation(opt) => {}
                Some(opt) => kept.extend(["-C".to_owned(), opt.to_owned()]),
                None => kept.push(flag.to_owned()),
            }
        } else if flag.strip_prefix("-C").is_some_and(is_instrumentation) {
        } else {
            kept.push(flag.to_owned());
        }
    }
    kept
}

/// Toolchain facts for the sysroot remap: `rustc --print sysroot` and the
/// `commit-hash:` line of `rustc -vV`.
pub struct Toolchain<'a> {
    pub sysroot: &'a Path,
    pub commit_hash: &'a str,
}

/// The `commit-hash:` value of `rustc -vV`, if present and non-empty.
pub fn rustc_commit_hash(version_verbose: &str) -> Option<&str> {
    version_verbose
        .lines()
        .find_map(|l| l.strip_prefix("commit-hash:"))
        .map(str::trim)
        .filter(|h| !h.is_empty() && *h != "unknown")
}

/// `CARGO_ENCODED_RUSTFLAGS` for the nested build: the outer flags minus
/// instrumentation, plus remaps that keep `$CARGO_HOME`, the checkout path and
/// the rust-src paths under the sysroot out of the launcher's panic and debug
/// strings. The sysroot maps to `/rustc/<hash>`, where the prebuilt std already
/// records its own paths; without a toolchain the sysroot paths stay as they are.
pub fn nested_rustflags(
    outer_encoded: &str,
    cargo_home: Option<&Path>,
    root: &Path,
    toolchain: Option<Toolchain<'_>>,
) -> String {
    let mut flags = filter_rustflags(outer_encoded);
    let sysroot = toolchain.map(|t| (t.sysroot, format!("/rustc/{}", t.commit_hash)));
    let remaps = [
        (cargo_home, "/cargo"),
        (Some(root), "/grimoire"),
        (
            sysroot.as_ref().map(|(from, _)| *from),
            sysroot.as_ref().map_or("", |(_, to)| to),
        ),
    ];
    for (from, to) in remaps {
        if let Some(from) = from {
            flags.push(format!("--remap-path-prefix={}={to}", from.display()));
        }
    }
    flags.join(&ENCODED_FLAG_SEP.to_string())
}

/// Environment edits for the nested `cargo build`.
pub struct NestedEnv {
    pub remove: Vec<OsString>,
    pub set: Vec<(&'static str, String)>,
}

/// Which outer variables the nested build drops and which it sets. Apply
/// `remove` before `set`: `CARGO_ENCODED_RUSTFLAGS` is on the deny list, so the
/// reverse order erases the nested flags and the launcher keeps `$CARGO_HOME` paths.
pub fn nested_env(outer: impl IntoIterator<Item = (OsString, OsString)>, rustflags: String) -> NestedEnv {
    let remove = outer
        .into_iter()
        .filter(|(name, value)| match (name.to_str(), value.to_str()) {
            (Some(n), Some(v)) => is_denied_env(n, v),
            (Some(n), None) => is_denied_env(n, ""),
            _ => false,
        })
        .map(|(name, _)| name)
        .collect();
    NestedEnv {
        remove,
        set: vec![("CARGO_ENCODED_RUSTFLAGS", rustflags)],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn denylist_strips_every_listed_variable() {
        for name in [
            "CARGO_PKG_NAME",
            "CARGO_MANIFEST_DIR",
            "CARGO_MANIFEST_LINKS",
            "CARGO_CFG_TARGET_OS",
            "CARGO_FEATURE_FOO",
            "CARGO_PRIMARY_PACKAGE",
            "OUT_DIR",
            "TARGET",
            "HOST",
            "PROFILE",
            "OPT_LEVEL",
            "DEBUG",
            "NUM_JOBS",
            "RUSTC",
            "RUSTDOC",
            "RUSTC_LINKER",
            "CARGO_ENCODED_RUSTFLAGS",
            "RUSTC_WRAPPER",
            "RUSTC_WORKSPACE_WRAPPER",
            "LLVM_PROFILE_FILE",
            "CARGO_LLVM_COV",
            "CARGO_LLVM_COV_TARGET_DIR",
        ] {
            assert!(is_denied_env(name, ""), "{name} must be stripped");
        }
    }

    #[test]
    fn workspace_wrapper_is_forwarded_only_for_cargo_auditable() {
        for name in ["RUSTC_WORKSPACE_WRAPPER", "CARGO_BUILD_RUSTC_WORKSPACE_WRAPPER"] {
            for value in [
                "/home/u/.cargo/bin/cargo-auditable",
                "cargo-auditable",
                "/home/u/.cargo/bin/cargo-auditable.exe",
            ] {
                assert!(!is_denied_env(name, value), "{name}={value} must be forwarded");
            }
            for value in [
                "",
                "clippy-driver",
                "/usr/bin/sccache",
                "/x/Cargo-Auditable",
                "/x/cargo-auditable.sh",
                "/x/not-cargo-auditable",
                "/x/cargo-auditable/clippy-driver",
            ] {
                assert!(is_denied_env(name, value), "{name}={value} must be stripped");
            }
        }
    }

    #[test]
    fn rustc_wrapper_is_stripped_even_for_cargo_auditable() {
        for name in ["RUSTC_WRAPPER", "CARGO_BUILD_RUSTC_WRAPPER"] {
            assert!(is_denied_env(name, "/bin/cargo-auditable"), "{name}");
        }
    }

    #[test]
    fn auditable_handoff_variable_is_forwarded() {
        assert!(!is_denied_env("CARGO_AUDITABLE_ORIG_ARGS", "build --release"));
    }

    #[test]
    fn denylist_keeps_toolchain_and_jobserver_variables() {
        for name in [
            "CARGO_MAKEFLAGS",
            "CARGO_HOME",
            "CARGO_NET_OFFLINE",
            "CC_x86_64_unknown_linux_gnu",
            "AR_aarch64_apple_darwin",
            "CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER",
            "PATH",
            "SystemRoot",
            "RUSTUP_TOOLCHAIN",
        ] {
            assert!(!is_denied_env(name, ""), "{name} must be forwarded");
        }
    }

    #[test]
    fn profile_follows_the_outer_profile() {
        assert_eq!(nested_profile("release"), "hook");
        assert_eq!(nested_profile("debug"), "hook-dev");
    }

    #[test]
    fn target_dir_from_unset_env_walks_up_from_out_dir() {
        let root = Path::new("/w");
        let triple = "x86_64-unknown-linux-gnu";
        for out in [
            "/w/target/debug/build/grimoire-abc/out",
            "/w/target/x86_64-unknown-linux-gnu/release/build/grimoire-abc/out",
        ] {
            assert_eq!(
                embed_target_dir(None, root, Path::new(out), triple),
                Some(PathBuf::from("/w/target/grim-hook-embed")),
                "{out}"
            );
        }
        assert_eq!(
            embed_target_dir(Some(""), root, Path::new("/w/t/debug/build/g-1/out"), triple),
            Some(PathBuf::from("/w/t/grim-hook-embed"))
        );
    }

    #[test]
    fn target_dir_from_absolute_env_is_used_verbatim() {
        let dir = embed_target_dir(Some("/cache/t"), Path::new("/w"), Path::new("/ignored"), "t");
        assert_eq!(dir, Some(PathBuf::from("/cache/t/grim-hook-embed")));
    }

    #[test]
    fn target_dir_from_relative_env_resolves_against_the_package_root() {
        let dir = embed_target_dir(Some("target/xwin"), Path::new("/w"), Path::new("/x"), "t");
        assert_eq!(dir, Some(PathBuf::from("/w/target/xwin/grim-hook-embed")));
    }

    #[test]
    fn target_dir_is_none_when_out_dir_is_too_shallow() {
        assert_eq!(embed_target_dir(None, Path::new("/w"), Path::new("/out"), "t"), None);
    }

    #[test]
    fn args_pin_profile_target_dir_and_lockfile() {
        let args = nested_args("hook", "x86_64-pc-windows-msvc", Path::new("/t/e"), false);
        assert_eq!(
            args.join(" "),
            "build -p grim-hook --profile hook --target x86_64-pc-windows-msvc --locked --target-dir /t/e"
        );
        assert_eq!(
            nested_args("hook", "t", Path::new("/t"), true).last().unwrap(),
            "--offline"
        );
    }

    #[test]
    fn truthy_values_match_cargo_bool_env() {
        for v in ["1", "true", "TRUE", "yes", "on"] {
            assert!(is_truthy(v), "{v}");
        }
        for v in ["", "0", "false", "off"] {
            assert!(!is_truthy(v), "{v}");
        }
    }

    #[test]
    fn rustflags_filter_drops_instrumentation_in_both_spellings() {
        let encoded = [
            "-Cinstrument-coverage",
            "-C",
            "instrument-coverage",
            "-Cprofile-generate=/tmp/p",
            "-C",
            "profile-generate",
            "-C",
            "target-cpu=native",
            "-Ctarget-feature=+crt-static",
            "--cfg=keep",
        ]
        .join("\x1f");
        assert_eq!(
            filter_rustflags(&encoded),
            ["-C", "target-cpu=native", "-Ctarget-feature=+crt-static", "--cfg=keep"]
        );
        assert!(filter_rustflags("").is_empty());
    }

    #[test]
    fn nested_rustflags_appends_path_remaps() {
        let flags = nested_rustflags(
            "-Cinstrument-coverage\x1f--cfg=x",
            Some(Path::new("/home/u/.cargo")),
            Path::new("/w"),
            None,
        );
        assert_eq!(
            flags,
            "--cfg=x\x1f--remap-path-prefix=/home/u/.cargo=/cargo\x1f--remap-path-prefix=/w=/grimoire"
        );
    }

    #[test]
    fn nested_rustflags_remaps_the_sysroot_to_the_rustc_commit() {
        let flags = nested_rustflags(
            "",
            None,
            Path::new("/w"),
            Some(Toolchain {
                sysroot: Path::new("/home/u/.rustup/toolchains/stable"),
                commit_hash: "abc123",
            }),
        );
        assert_eq!(
            flags,
            "--remap-path-prefix=/w=/grimoire\x1f--remap-path-prefix=/home/u/.rustup/toolchains/stable=/rustc/abc123"
        );
    }

    #[test]
    fn commit_hash_is_read_from_rustc_version_output() {
        let out = "rustc 1.95.0 (abc 2026-01-01)\nbinary: rustc\ncommit-hash: 59807616e1fa\nhost: x\n";
        assert_eq!(rustc_commit_hash(out), Some("59807616e1fa"));
        assert_eq!(rustc_commit_hash("commit-hash: unknown\n"), None);
        assert_eq!(rustc_commit_hash("commit-hash:  \n"), None);
        assert_eq!(rustc_commit_hash("host: x\n"), None);
    }

    #[test]
    fn nested_env_keeps_the_nested_rustflags_and_drops_denied_variables() {
        let outer = [
            ("CARGO_ENCODED_RUSTFLAGS", "-Cinstrument-coverage"),
            ("OUT_DIR", "/o"),
            ("RUSTC_WRAPPER", "sccache"),
            ("PATH", "/bin"),
        ]
        .map(|(k, v)| (OsString::from(k), OsString::from(v)));
        let env = nested_env(outer, "--remap-path-prefix=/a=/b".to_owned());
        assert_eq!(
            env.set,
            [("CARGO_ENCODED_RUSTFLAGS", "--remap-path-prefix=/a=/b".to_owned())]
        );
        assert!(env.remove.contains(&OsString::from("CARGO_ENCODED_RUSTFLAGS")));
        assert!(env.remove.contains(&OsString::from("OUT_DIR")));
        assert!(env.remove.contains(&OsString::from("RUSTC_WRAPPER")));
        assert!(!env.remove.contains(&OsString::from("PATH")));
    }
}
