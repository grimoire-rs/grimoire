// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

/// `GRIM_BIN` when it is set and absolute, else `grim` on `PATH`.
pub fn resolve_bin(grim_bin: Option<&OsStr>) -> PathBuf {
    match grim_bin.map(Path::new) {
        Some(p) if p.is_absolute() => p.to_path_buf(),
        _ => PathBuf::from("grim"),
    }
}

/// [`resolve_bin`] over the process environment.
pub fn bin_from_env() -> PathBuf {
    resolve_bin(std::env::var_os("GRIM_BIN").as_deref())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_bin_takes_only_absolute_grim_bin() {
        assert_eq!(resolve_bin(Some(OsStr::new("/opt/grim"))), PathBuf::from("/opt/grim"));
        assert_eq!(resolve_bin(Some(OsStr::new("rel/grim"))), PathBuf::from("grim"));
        assert_eq!(resolve_bin(Some(OsStr::new(""))), PathBuf::from("grim"));
        assert_eq!(resolve_bin(None), PathBuf::from("grim"));
    }

    #[cfg(unix)]
    #[test]
    fn resolve_bin_keeps_a_non_utf8_absolute_path() {
        use std::os::unix::ffi::OsStrExt;
        let raw = OsStr::from_bytes(b"/opt/gr\xffim");
        assert_eq!(resolve_bin(Some(raw)), PathBuf::from(raw));
    }

    // set_var is unsafe and the crate forbids unsafe: re-run this test binary with the variable set.
    #[test]
    fn bin_from_env_reads_grim_bin() {
        if std::env::var_os("SDK_BIN_CHILD").is_some() {
            assert_eq!(bin_from_env(), PathBuf::from("/opt/from-env/grim"));
            return;
        }
        let out = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "bin::tests::bin_from_env_reads_grim_bin"])
            .env("SDK_BIN_CHILD", "1")
            .env("GRIM_BIN", "/opt/from-env/grim")
            .output()
            .unwrap();
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stdout));
    }
}
