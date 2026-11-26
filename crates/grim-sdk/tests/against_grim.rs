// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! Run with `GRIM_SDK_TEST_GRIM=<absolute path to grim> cargo test -p grim-sdk
//! --test against_grim -- --ignored`.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::PathBuf;
use std::process::Command;

use grim_sdk::GrimDriver;

#[test]
#[ignore = "needs a built grim: set GRIM_SDK_TEST_GRIM"]
fn driver_agrees_with_the_real_grim() {
    let bin = std::env::var_os("GRIM_SDK_TEST_GRIM")
        .map(PathBuf::from)
        .expect("GRIM_SDK_TEST_GRIM must be set to the absolute path of a grim binary");
    assert!(
        bin.is_absolute(),
        "GRIM_SDK_TEST_GRIM must be absolute: {}",
        bin.display()
    );
    let d = GrimDriver { bin: bin.clone() };

    let printed = Command::new(&bin).arg("--version").output().unwrap();
    let text = String::from_utf8(printed.stdout).unwrap();
    assert_eq!(d.version().unwrap(), text.trim().strip_prefix("grim ").unwrap());

    let printed_version = semver::Version::parse(text.trim().strip_prefix("grim ").unwrap()).unwrap();
    assert_eq!(d.require_version(&printed_version).unwrap(), printed_version);
    assert!(d.require_version(&semver::Version::new(0, 17, 0)).is_ok());
    let e = d.require_version(&semver::Version::new(99, 0, 0)).unwrap_err();
    assert_eq!(e.code, Some(69));

    assert!(d.has_command("convert").unwrap());
    assert!(!d.has_command("no-such-command").unwrap());
    assert!(d.schema("publish").unwrap().is_object());
}
