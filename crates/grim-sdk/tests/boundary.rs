// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! The crate must stay movable and independent: it links no grim crate,
//! is never published or distributed, and keeps its own version line.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::PathBuf;
use std::process::Command;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn manifest() -> toml::Table {
    std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml"))
        .unwrap()
        .parse()
        .unwrap()
}

#[test]
fn dependency_tree_has_no_grim_package() {
    let out = Command::new(env!("CARGO"))
        .args([
            "tree",
            "-p",
            "grim-sdk",
            "-e",
            "normal",
            "--prefix",
            "none",
            "--offline",
        ])
        .current_dir(root())
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let tree = String::from_utf8(out.stdout).unwrap();
    let names: Vec<&str> = tree.lines().filter_map(|l| l.split_whitespace().next()).collect();
    assert!(names.contains(&"grim-sdk"), "{tree}");
    for banned in ["grimoire", "grim-hook", "grimoire-hook-core"] {
        assert!(
            !names.contains(&banned),
            "grim-sdk must not depend on {banned}:\n{tree}"
        );
    }
}

#[test]
fn manifest_is_not_published() {
    assert_eq!(manifest()["package"]["publish"].as_bool(), Some(false));
}

#[test]
fn dist_workspace_does_not_name_the_crate() {
    let dist = std::fs::read_to_string(root().join("dist-workspace.toml")).unwrap();
    assert!(!dist.contains("grim-sdk"));
}

#[test]
fn version_is_a_literal_not_the_workspace_line() {
    assert_eq!(manifest()["package"]["version"].as_str(), Some("0.1.0"));
}

#[test]
fn release_bump_touches_only_the_root_package() {
    let task = std::fs::read_to_string(root().join("taskfiles/release.taskfile.yml")).unwrap();
    let start = task
        .lines()
        .position(|l| l.contains("cargo set-version \"") || l.contains("cargo set-version {{"))
        .expect("no `cargo set-version <version>` line in release.taskfile.yml");
    // A multi-line `cmds:` entry: join its continuation lines up to the next `- ` item.
    let mut joined = String::new();
    for (n, l) in task.lines().skip(start).enumerate() {
        if n > 0 && (l.trim_start().starts_with("- ") || l.trim().is_empty()) {
            break;
        }
        joined.push_str(l);
        joined.push(' ');
    }
    assert!(!joined.contains("--workspace"), "{joined}");
    assert!(!joined.contains(" -p ") && !joined.contains("--package"), "{joined}");
}
