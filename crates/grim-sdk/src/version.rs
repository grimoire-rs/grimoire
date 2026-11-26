// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

use std::ffi::OsStr;

use crate::driver::GrimDriver;
use crate::error::{DriverError, unexpected};
use crate::process::{CALL_DEADLINE, run};

impl GrimDriver {
    /// `grim --version` without the `grim ` prefix.
    ///
    /// # Errors
    ///
    /// grim missing or exiting non-zero.
    pub fn version(&self) -> Result<String, DriverError> {
        let v = run(&self.bin, &[OsStr::new("--version")], None, &[], CALL_DEADLINE)?;
        if v.code != Some(0) {
            return Err(unexpected("--version", &v));
        }
        let text = v.stdout.trim();
        Ok(text.strip_prefix("grim ").unwrap_or(text).to_owned())
    }

    /// The installed version when it is at least `min`.
    ///
    /// # Errors
    ///
    /// Exit 69 when grim is older than `min` or prints no parsable version;
    /// no code when grim cannot be run.
    pub fn require_version(&self, min: &semver::Version) -> Result<semver::Version, DriverError> {
        let text = self.version()?;
        let found = semver::Version::parse(&text).map_err(|_| DriverError {
            code: Some(69),
            message: format!("unexpected grim output: --version {text:?}"),
        })?;
        if &found < min {
            return Err(DriverError {
                code: Some(69),
                message: format!("grim {found} is older than {min}; upgrade grim"),
            });
        }
        Ok(found)
    }
}

#[cfg(all(test, unix))]
mod tests {
    use std::path::PathBuf;

    use semver::Version;

    use super::*;
    use crate::fake::Fake;

    fn v(s: &str) -> Version {
        Version::parse(s).unwrap()
    }

    fn with_version(out: &str) -> Fake {
        let f = Fake::new();
        f.set("--version.out", out);
        f
    }

    #[test]
    fn version_strips_the_prefix() {
        assert_eq!(Fake::new().driver().version().unwrap(), "0.17.0");
    }

    #[test]
    fn equal_version_passes() {
        let f = with_version("grim 0.17.0\n");
        assert_eq!(f.driver().require_version(&v("0.17.0")).unwrap(), v("0.17.0"));
    }

    #[test]
    fn above_the_floor_passes() {
        let f = with_version("grim 1.2.3\n");
        assert_eq!(f.driver().require_version(&v("0.17.0")).unwrap(), v("1.2.3"));
    }

    #[test]
    fn below_the_floor_is_69_and_says_upgrade() {
        let f = with_version("grim 0.16.9\n");
        let e = f.driver().require_version(&v("0.17.0")).unwrap_err();
        assert_eq!(e.code, Some(69));
        assert_eq!(e.message, "grim 0.16.9 is older than 0.17.0; upgrade grim");
    }

    #[test]
    fn pre_release_is_below_its_release() {
        let f = with_version("grim 0.18.0-rc.1\n");
        let e = f.driver().require_version(&v("0.18.0")).unwrap_err();
        assert_eq!(e.code, Some(69));
        assert!(e.message.contains("older"), "{}", e.message);
        assert!(f.driver().require_version(&v("0.17.0")).is_ok());
    }

    #[test]
    fn garbage_output_is_69_unexpected() {
        let f = with_version("not a version\n");
        let e = f.driver().require_version(&v("0.1.0")).unwrap_err();
        assert_eq!(e.code, Some(69));
        assert!(e.message.contains("unexpected grim output"), "{}", e.message);
    }

    #[test]
    fn missing_binary_has_no_code() {
        let d = GrimDriver {
            bin: PathBuf::from("/nonexistent/grim"),
        };
        assert_eq!(d.require_version(&v("0.1.0")).unwrap_err().code, None);
    }
}
