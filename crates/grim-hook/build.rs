// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! Link hygiene for the launcher (C-023). The root `build.rs` never reaches the
//! nested build's link line, so the flag is emitted here.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    // 0x800 = LOAD_LIBRARY_SEARCH_SYSTEM32: dependent DLLs resolve from System32 only,
    // never from the directory beside `grim-hook.exe`. link.exe and lld-link both take it.
    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        println!("cargo:rustc-link-arg-bins=/DEPENDENTLOADFLAG:0x800");
    }
}
