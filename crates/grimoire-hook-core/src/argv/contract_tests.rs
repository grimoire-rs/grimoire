//! Contract tests for C-N4, written from the grammar and not from `parse`'s body.

use std::ffi::OsString;

use super::*;

const ROOT: &str = "0123456789abcdef0123456789abcdef";

fn os(items: &[&str]) -> Vec<OsString> {
    items.iter().map(OsString::from).collect()
}

fn parse_str(client: &str, event: &str, root: &str) -> Result<Invocation, ArgvError> {
    parse(&os(&[client, event, root]))
}

#[test]
fn a_well_formed_triple_parses_to_the_same_operands() {
    let inv = parse_str("claude", "PreToolUse", ROOT).unwrap();
    assert_eq!(inv.client, "claude");
    assert_eq!(inv.event, "PreToolUse");
    assert_eq!(inv.root.as_str(), ROOT);
}

#[test]
fn render_returns_the_three_operands_in_order() {
    assert_eq!(
        render("claude", "PreToolUse", ROOT),
        ["claude".to_owned(), "PreToolUse".to_owned(), ROOT.to_owned()]
    );
}

#[test]
fn client_grammar_boundaries() {
    let max = format!("a{}", "b".repeat(CLIENT_MAX_BYTES - 1));
    assert!(parse_str(&max, "Stop", ROOT).is_ok(), "32 bytes is the maximum");
    let over = format!("a{}", "b".repeat(CLIENT_MAX_BYTES));
    assert_eq!(parse_str(&over, "Stop", ROOT), Err(ArgvError::Client));
    assert!(parse_str("a", "Stop", ROOT).is_ok(), "one letter is enough");
    assert!(parse_str("a-1", "Stop", ROOT).is_ok());
    for bad in ["", "1abc", "-abc", "Claude", "cl_aude", "cl.aude", "clàude", "cl aude"] {
        assert_eq!(parse_str(bad, "Stop", ROOT), Err(ArgvError::Client), "{bad:?}");
    }
}

#[test]
fn event_grammar_boundaries() {
    let max = format!("A{}", "b".repeat(EVENT_MAX_BYTES - 1));
    assert!(parse_str("claude", &max, ROOT).is_ok(), "64 bytes is the maximum");
    let over = format!("A{}", "b".repeat(EVENT_MAX_BYTES));
    assert_eq!(parse_str("claude", &over, ROOT), Err(ArgvError::Event));
    for ok in ["a", "Z", "pre.tool-use_1", "PermissionRequest", "e1.2", "a--", "a.."] {
        assert!(parse_str("claude", ok, ROOT).is_ok(), "{ok:?}");
    }
    for bad in [
        "", "1e10", "9", ".x", "-x", "_x", "Pre Tool", "Pre/Tool", "Pre:Tool", "Évent",
    ] {
        assert_eq!(parse_str("claude", bad, ROOT), Err(ArgvError::Event), "{bad:?}");
    }
}

#[test]
fn an_event_that_maps_to_no_canonical_event_is_not_an_error() {
    let inv = parse_str("claude", "PermissionRequest", ROOT).unwrap();
    assert_eq!(inv.event, "PermissionRequest");
    assert!(crate::canonical_event("NotARealEvent").is_none());
    assert!(parse_str("claude", "NotARealEvent", ROOT).is_ok());
}

#[test]
fn root_grammar_boundaries() {
    assert!(parse_str("claude", "Stop", &"f".repeat(32)).is_ok());
    assert_eq!(parse_str("claude", "Stop", &"f".repeat(31)), Err(ArgvError::Root));
    assert_eq!(parse_str("claude", "Stop", &"f".repeat(33)), Err(ArgvError::Root));
    assert_eq!(parse_str("claude", "Stop", &"F".repeat(32)), Err(ArgvError::Root));
    assert_eq!(
        parse_str("claude", "Stop", "0123456789ABCDEF0123456789abcdef"),
        Err(ArgvError::Root)
    );
    assert_eq!(parse_str("claude", "Stop", &"g".repeat(32)), Err(ArgvError::Root));
    assert_eq!(parse_str("claude", "Stop", ""), Err(ArgvError::Root));
    assert_eq!(
        parse_str("claude", "Stop", &format!(" {}", &ROOT[1..])),
        Err(ArgvError::Root)
    );
}

#[test]
fn the_operand_count_is_exactly_three() {
    for n in [0, 1, 2, 4, 5] {
        let args: Vec<OsString> = ["claude", "Stop", ROOT, "extra", "more"]
            .iter()
            .take(n)
            .map(OsString::from)
            .collect();
        let n = args.len();
        assert_eq!(parse(&args), Err(ArgvError::OperandCount(n)), "{n} operands");
    }
}

#[test]
fn there_are_no_flags_and_no_end_of_options_marker() {
    for args in [
        os(&["--", "claude", "Stop", ROOT]),
        os(&["--", "Stop", ROOT]),
        os(&["claude", "--", ROOT]),
        os(&["claude", "Stop", "--"]),
        os(&["-x", "Stop", ROOT]),
        os(&["claude", "-x", ROOT]),
        os(&["claude", "Stop", "-x"]),
        os(&["--help"]),
        os(&["claude", "Stop", ROOT, "--"]),
    ] {
        assert!(parse(&args).is_err(), "{args:?} must be refused");
    }
}

#[cfg(unix)]
#[test]
fn a_non_utf8_operand_is_refused() {
    use std::os::unix::ffi::OsStringExt;
    let bad = OsString::from_vec(vec![b'a', 0xff, b'b']);
    let valid = [OsString::from("claude"), OsString::from("Stop"), OsString::from(ROOT)];
    for slot in 0..3 {
        let mut args = valid.to_vec();
        args[slot] = bad.clone();
        assert_eq!(parse(&args), Err(ArgvError::NotUtf8(slot)), "slot {slot}");
    }
}

/// Deterministic xorshift, so the property runs without a dependency.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn pick(&mut self, set: &[u8]) -> char {
        char::from(set[(self.next() % set.len() as u64) as usize])
    }

    fn string(&mut self, first: &[u8], rest: &[u8], max: usize) -> String {
        let len = 1 + (self.next() % max as u64) as usize;
        let mut out = String::from(self.pick(first));
        out.extend((1..len).map(|_| self.pick(rest)));
        out
    }
}

#[test]
fn render_then_parse_round_trips_generated_valid_triples() {
    let lower = b"abcdefghijklmnopqrstuvwxyz";
    let client_rest = b"abcdefghijklmnopqrstuvwxyz0123456789-";
    let upper_lower = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ";
    let event_rest = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789._-";
    let hex = b"0123456789abcdef";
    let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
    for _ in 0..500 {
        let client = rng.string(lower, client_rest, CLIENT_MAX_BYTES);
        let event = rng.string(upper_lower, event_rest, EVENT_MAX_BYTES);
        let root: String = (0..32).map(|_| rng.pick(hex)).collect();
        let args: Vec<OsString> = render(&client, &event, &root).into_iter().map(OsString::from).collect();
        let inv = parse(&args).unwrap_or_else(|e| panic!("{client} {event} {root}: {e}"));
        assert_eq!(
            (inv.client.as_str(), inv.event.as_str(), inv.root.as_str()),
            (client.as_str(), event.as_str(), root.as_str())
        );
    }
}

/// Numeric-looking tokens must arrive verbatim, never reinterpreted as numbers.
#[test]
fn numeric_looking_tokens_arrive_verbatim() {
    for root in [
        "00000000000000000000000000000000",
        "1e100000000000000000000000000000",
        "0e000000000000000000000000000000",
        "12345678901234567890123456789012",
    ] {
        assert_eq!(parse_str("claude", "e1e10", root).unwrap().root.as_str(), root);
    }
    // A leading digit is an invalid event: the first character must be a letter.
    assert_eq!(parse_str("claude", "1e10", ROOT), Err(ArgvError::Event));
    assert_eq!(parse_str("claude", "e1e10", ROOT).unwrap().event, "e1e10");
}

/// Whatever grim registers for a known client and event, the launcher parses
/// back to the same operands.
#[test]
fn every_client_and_event_the_core_knows_renders_and_parses_back_unchanged() {
    let mut clients: Vec<&str> = crate::RESPONSE_PROJECTION.iter().map(|row| row.client).collect();
    clients.sort_unstable();
    clients.dedup();
    assert!(clients.len() >= 4, "the projection table lost clients: {clients:?}");
    for client in clients {
        for event in crate::CanonicalEvent::ALL {
            let rendered = render(client, event.as_str(), ROOT);
            let args: Vec<OsString> = rendered.iter().map(OsString::from).collect();
            let inv = parse(&args).unwrap_or_else(|e| panic!("{client} {event:?}: {e}"));
            assert_eq!(
                (inv.client.as_str(), inv.event.as_str(), inv.root.as_str()),
                (client, event.as_str(), ROOT)
            );
        }
    }
}
