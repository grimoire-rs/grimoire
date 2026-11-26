// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! Test double: a shell script standing in for grim.

use std::fs;
use std::os::unix::fs::PermissionsExt;

use crate::GrimDriver;

/// Per subcommand, `<dir>/<sub>.out` is stdout, `<dir>/<sub>.code` the exit
/// code (default 0), `<dir>/<sub>.sleep` seconds to sleep, `<dir>/<sub>.codes`
/// a queue of exit codes consumed one per call.
pub struct Fake {
    pub dir: tempfile::TempDir,
}

impl Fake {
    pub fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let script = r#"#!/bin/sh
d="$(dirname "$0")"
sub="$1"
{ echo "== $sub"; echo "args: $*"; echo "cwd: $(pwd)"; echo "sde: ${SOURCE_DATE_EPOCH-unset}"; echo "ceil: ${GIT_CEILING_DIRECTORIES-unset}"; } >> "$d/calls.log"
[ -f "$d/$sub.sleep" ] && exec sleep "$(cat "$d/$sub.sleep")"
code=0
if [ -f "$d/$sub.codes" ]; then
  code="$(head -n1 "$d/$sub.codes")"
  tail -n +2 "$d/$sub.codes" > "$d/$sub.codes.tmp"; mv "$d/$sub.codes.tmp" "$d/$sub.codes"
elif [ -f "$d/$sub.code" ]; then code="$(cat "$d/$sub.code")"; fi
[ -f "$d/$sub.out" ] && cat "$d/$sub.out"
[ -f "$d/$sub.err" ] && cat "$d/$sub.err" >&2
exit "$code"
"#;
        let bin = dir.path().join("grim");
        fs::write(&bin, script).unwrap();
        fs::set_permissions(&bin, fs::Permissions::from_mode(0o755)).unwrap();
        let f = Self { dir };
        f.set("--version.out", "grim 0.17.0\n");
        f
    }

    pub fn set(&self, file: &str, body: &str) {
        fs::write(self.dir.path().join(file), body).unwrap();
    }

    pub fn driver(&self) -> GrimDriver {
        GrimDriver {
            bin: self.dir.path().join("grim"),
        }
    }

    pub fn log(&self) -> String {
        fs::read_to_string(self.dir.path().join("calls.log")).unwrap_or_default()
    }
}
