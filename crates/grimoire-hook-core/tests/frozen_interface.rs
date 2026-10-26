// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! Pins the launcher's frozen interface: argv grammar and exit numbers.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::ffi::OsString;

use grimoire_hook_core::DispatchExit;
use grimoire_hook_core::argv::{ArgvError, parse};

const ROOT: &str = "0123456789abcdef0123456789abcdef";

/// One operand's grammar, `first rest{0,max-1}`, as data rather than as a regex.
struct Operand {
    first: &'static str,
    rest: &'static str,
    max: usize,
}

const LOWER: &str = "abcdefghijklmnopqrstuvwxyz";
const UPPER: &str = "ABCDEFGHIJKLMNOPQRSTUVWXYZ";
const DIGITS: &str = "0123456789";

fn os(items: &[&str]) -> Vec<OsString> {
    items.iter().map(OsString::from).collect()
}

/// Every ASCII byte is accepted in `slot` iff the grammar names it, and the length bound is exact.
fn pin(spec: &Operand, err: ArgvError, with: impl Fn(&str) -> Vec<OsString>) {
    let rest_ok = |s: &str| parse(&with(s)).is_ok();
    for b in 0u8..128 {
        let c = char::from(b);
        assert_eq!(rest_ok(&format!("{c}")), spec.first.contains(c), "first byte {b:#04x}");
        assert_eq!(rest_ok(&format!("a{c}")), spec.rest.contains(c), "later byte {b:#04x}");
    }
    let longest = format!("a{}", "b".repeat(spec.max - 1));
    assert!(rest_ok(&longest), "{} bytes is the maximum", spec.max);
    assert_eq!(parse(&with(&format!("{longest}b"))), Err(err), "one byte over");
    assert!(!rest_ok(""), "empty is refused");
}

/// The launcher's frozen interface (C-N4, C-N7): three operands, no flags, these
/// grammars and these exit numbers, and no exit 2. They are frozen at graduation and
/// treated as frozen now. Only new client/event literals are added; capability
/// arrives as additive optional `dispatch.json` fields, never argv. The launcher skips
/// rows and roots that are not addressed to the invocation, even when it cannot read
/// them, which keeps an older launcher safe against a newer table. A row that *is*
/// addressed and cannot be read exits 65, because skipping it could drop a gatekeeper. At graduation these move into `stability.md`'s frozen list.
/// See `adr_hook_native_dispatcher.md#c-n4`.
#[test]
fn frozen_interface() {
    // client `[a-z][a-z0-9-]{0,31}`
    let client = Operand {
        first: LOWER,
        rest: "abcdefghijklmnopqrstuvwxyz0123456789-",
        max: 32,
    };
    pin(&client, ArgvError::Client, |s| os(&[s, "Stop", ROOT]));
    // event `[A-Za-z][A-Za-z0-9._-]{0,63}`
    let event = Operand {
        first: "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ",
        rest: "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789._-",
        max: 64,
    };
    pin(&event, ArgvError::Event, |s| os(&["claude", s, ROOT]));
    // root `[0-9a-f]{32}`: exact length, lowercase hex only.
    let hex = format!("{DIGITS}abcdef");
    for c in (0u8..128).map(char::from) {
        let token = c.to_string().repeat(32);
        assert_eq!(
            parse(&os(&["claude", "Stop", &token])).is_ok(),
            hex.contains(c),
            "root byte {c:?}"
        );
    }
    for bad in [&ROOT[..31], &format!("{ROOT}0"), &ROOT.to_uppercase(), ""] {
        assert_eq!(parse(&os(&["claude", "Stop", bad])), Err(ArgvError::Root), "{bad:?}");
    }
    assert!(UPPER.chars().all(|c| !hex.contains(c)));

    // Exactly three operands; flags and `--` are ordinary operands and fail the grammar.
    for n in [0, 1, 2, 4] {
        let args = vec!["claude"; n];
        assert_eq!(parse(&os(&args)), Err(ArgvError::OperandCount(n)));
    }
    for flag in ["--", "-x", "--help"] {
        assert!(parse(&os(&[flag, "Stop", ROOT])).is_err());
        assert!(parse(&os(&["claude", flag, ROOT])).is_err());
        assert!(parse(&os(&["claude", "Stop", flag])).is_err());
    }

    // Exit numbers, with no 2 (Claude reads exit 2 as a block).
    let table: Vec<u8> = DispatchExit::ALL.iter().map(|e| e.code()).collect();
    assert_eq!(table, [0, 1, 64, 65, 74, 79]);
    assert_eq!(DispatchExit::ALL.len(), 6, "a new exit is a contract change");
}
