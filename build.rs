// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! Builds the `grim-hook` launcher with a nested cargo invocation and exposes
//! the artifact to `include_bytes!` as `GRIM_HOOK_BIN` (C-N2).
//!
//! `grim-hook` is deliberately not a cargo dependency of `grim`: it needs its
//! own size-tuned profile, which a dependency edge cannot select.

#[path = "build/embed.rs"]
mod embed;

use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    if let Err(message) = run() {
        eprintln!("{message}");
        std::process::exit(1);
    }
}

fn var(name: &str) -> Result<String, String> {
    println!("cargo:rerun-if-env-changed={name}");
    env::var(name).map_err(|_| format!("build.rs: {name} is not set"))
}

/// Sysroot and commit hash of the `RUSTC` this script was given; `None` when
/// either is unavailable, which leaves the sysroot paths unremapped.
fn rustc_toolchain() -> Option<(PathBuf, String)> {
    let rustc = env::var_os("RUSTC")?;
    let run = |arg: &str| {
        let out = Command::new(&rustc).args(arg.split(' ')).output().ok()?;
        out.status
            .success()
            .then(|| String::from_utf8(out.stdout).ok())
            .flatten()
    };
    let sysroot = run("--print sysroot")?;
    let hash = embed::rustc_commit_hash(&run("-vV")?)?.to_owned();
    Some((PathBuf::from(sysroot.trim_end()), hash))
}

fn run() -> Result<(), String> {
    let manifest_dir = PathBuf::from(var("CARGO_MANIFEST_DIR")?);
    let out_dir = PathBuf::from(var("OUT_DIR")?);
    let target = var("TARGET")?;
    let profile = embed::nested_profile(&var("PROFILE")?);
    let outer_flags = var("CARGO_ENCODED_RUSTFLAGS").unwrap_or_default();
    let cargo = env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let cargo_target_dir = env::var("CARGO_TARGET_DIR").ok();
    let offline = env::var("CARGO_NET_OFFLINE").is_ok_and(|v| embed::is_truthy(&v));
    let cargo_home = env::var_os("CARGO_HOME").map(PathBuf::from).or_else(|| {
        env::var_os("HOME")
            .or_else(|| env::var_os("USERPROFILE"))
            .map(|h| Path::new(&h).join(".cargo"))
    });

    // Variables whose value changes the nested build, beyond the ones `var` tracks.
    let triple_env = target.replace('-', "_");
    for name in [
        "CARGO_TARGET_DIR".to_owned(),
        "CARGO_NET_OFFLINE".to_owned(),
        "CARGO_HOME".to_owned(),
        "RUSTFLAGS".to_owned(),
        "RUSTUP_TOOLCHAIN".to_owned(),
        "RUSTC_WORKSPACE_WRAPPER".to_owned(),
        "CARGO_BUILD_RUSTC_WORKSPACE_WRAPPER".to_owned(),
        "CARGO_AUDITABLE_ORIG_ARGS".to_owned(),
        format!("CC_{triple_env}"),
        format!("AR_{triple_env}"),
        format!("CARGO_TARGET_{}_LINKER", triple_env.to_ascii_uppercase()),
    ] {
        println!("cargo:rerun-if-env-changed={name}");
    }

    // A path that does not exist makes cargo rerun the script on every build.
    for path in [
        "crates",
        "Cargo.toml",
        "Cargo.lock",
        ".cargo/config.toml",
        "rust-toolchain.toml",
    ] {
        if manifest_dir.join(path).exists() {
            println!("cargo:rerun-if-changed={path}");
        }
    }

    let target_dir = embed::embed_target_dir(cargo_target_dir.as_deref(), &manifest_dir, &out_dir, &target)
        .ok_or("build.rs: cannot derive the outer target dir from OUT_DIR")?;

    let toolchain = rustc_toolchain();
    let rustflags = embed::nested_rustflags(
        &outer_flags,
        cargo_home.as_deref(),
        &manifest_dir,
        toolchain.as_ref().map(|(sysroot, hash)| embed::Toolchain {
            sysroot,
            commit_hash: hash,
        }),
    );
    let nested_env = embed::nested_env(env::vars_os(), rustflags);

    let mut command = Command::new(&cargo);
    command
        .args(embed::nested_args(profile, &target, &target_dir, offline))
        .current_dir(&manifest_dir);
    for name in nested_env.remove {
        command.env_remove(name);
    }
    for (name, value) in nested_env.set {
        command.env(name, value);
    }

    let output = command
        .output()
        .map_err(|e| format!("build.rs: cannot run {cargo:?}: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "build.rs: nested `cargo {}` failed ({})\n{}",
            embed::nested_args(profile, &target, &target_dir, offline).join(" "),
            output.status,
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    let exe = if target.contains("windows") {
        "grim-hook.exe"
    } else {
        "grim-hook"
    };
    let built = target_dir.join(&target).join(profile).join(exe);
    let embedded = out_dir.join(exe);
    std::fs::copy(&built, &embedded).map_err(|e| {
        format!(
            "build.rs: cannot copy {} to {}: {e}",
            built.display(),
            embedded.display()
        )
    })?;
    println!("cargo:rustc-env=GRIM_HOOK_BIN={}", embedded.display());
    Ok(())
}
