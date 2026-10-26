// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! Structural guards on the launcher: its dependency set (C-014) and the one
//! environment variable it may read (C-013). Both are source/manifest facts, so
//! they run on every platform and never execute the binary.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;
use std::process::Command;

/// Everything the launcher may link. A new entry widens the supply chain of a binary
/// that runs on every hook event, so it is a reviewed change, not a convenience.
const ALLOWED: &[&str] = &[
    "grim-hook",
    "grimoire-hook-core",
    "serde",
    "serde_core",
    "serde_derive",
    "serde_json",
    "itoa",
    "memchr",
    "zmij",
    "proc-macro2",
    "quote",
    "syn",
    "unicode-ident",
];

/// `cargo tree -p` resolves features for the launcher alone, so grim's `schema`
/// feature on the shared core crate (which pulls `schemars`) does not leak in the
/// way it would through workspace-wide `cargo metadata`.
fn normal_dependencies() -> BTreeSet<String> {
    let out = Command::new(env!("CARGO"))
        .args(["tree", "-p", "grim-hook", "-e", "normal", "--target", "all"])
        .args(["--prefix", "none", "--format", "{p}", "--locked", "--offline"])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("run cargo tree");
    assert!(
        out.status.success(),
        "cargo tree failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout)
        .expect("utf-8")
        .lines()
        .filter_map(|line| line.split_whitespace().next())
        .map(str::to_owned)
        .collect()
}

#[test]
fn allowlist_holds_the_launchers_normal_dependencies() {
    let deps = normal_dependencies();
    let allowed: BTreeSet<String> = ALLOWED.iter().map(|s| (*s).to_owned()).collect();
    let extra: Vec<_> = deps.difference(&allowed).collect();
    assert!(
        extra.is_empty(),
        "grim-hook links crates outside the allowlist: {extra:?}"
    );
    assert!(!deps.contains("schemars"), "schemars must never reach the launcher");
}

fn rust_files(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// Production code only: a file's `#[cfg(test)]` tail may set variables for its own checks.
fn production(source: &str) -> &str {
    source.split_once("#[cfg(test)]").map_or(source, |(before, _)| before)
}

#[test]
fn env_var_reads_are_limited_to_grim_log() {
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut files = Vec::new();
    for member in ["grim-hook/src", "grimoire-hook-core/src"] {
        rust_files(&crates.join(member), &mut files);
    }
    assert!(
        files.len() > 10,
        "scan found only {} files; the path is wrong",
        files.len()
    );

    for file in files {
        let source = fs::read_to_string(&file).unwrap();
        for (n, line) in production(&source).lines().enumerate() {
            if line.trim_start().starts_with("//") {
                continue;
            }
            let reads_env = ["env::var(", "env::var_os(", "env::vars(", "env::vars_os("]
                .iter()
                .any(|needle| line.contains(needle));
            assert!(
                !reads_env || line.contains("\"GRIM_LOG\""),
                "{}:{}: the launcher may read only GRIM_LOG from the environment (C-013): {}",
                file.display(),
                n + 1,
                line.trim()
            );
        }
    }
}

/// A panic must never unwind out of a launcher a client runs on every tool call:
/// the launcher profiles abort instead. Read from the workspace manifest, since
/// the profile is not visible to the compiled binary.
#[test]
fn the_launcher_profiles_abort_on_panic() {
    let manifest = fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../Cargo.toml")).unwrap();
    for profile in ["profile.hook", "profile.hook-dev"] {
        let header = format!("[{profile}]");
        let section = manifest
            .split_once(&header)
            .unwrap_or_else(|| panic!("{header} is missing from the workspace Cargo.toml"))
            .1
            .split("\n[")
            .next()
            .unwrap();
        assert!(
            section.lines().any(|l| l.trim() == "panic = \"abort\""),
            "{header} must set panic = \"abort\""
        );
    }
}
