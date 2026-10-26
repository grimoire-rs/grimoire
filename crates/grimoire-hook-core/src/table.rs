// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! The dispatch table's types and its one reader.

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::canonical_event;
use crate::diag::Diagnostic;
use crate::{CanonicalEvent, HookHandler, HookPayloadMode, HookTier, MATCHER_MAX_BYTES};

/// The dispatch table's schema version, read **before** anything else.
///
/// An unrecognized value, including a *newer* one after a grim downgrade, makes
/// [`parse_table`] return the empty table with [`DispatchDegrade::UnknownSchema`]
/// (grim's readers: "nothing armed"), and makes [`select`] refuse the table with
/// [`TableError::Schema`] (the launcher: exit 65, never a forged success).
pub const DISPATCH_SCHEMA: u32 = 1;

/// Hard ceiling on the table read (W2 (c)).
///
/// A build-time cap does not bind a file on disk, so the runtime re-checks both
/// this and `MATCHER_MAX_BYTES` at read time. 1 MiB is far above any real table
/// (one JSON object per armed hook) and far below anything that costs the hot
/// path a measurable read.
pub const MAX_TABLE_BYTES: u64 = 1 << 20;

/// An opaque per-install root key: `HMAC-SHA256(machine key, root)`, hex,
/// truncated to 128 bits.
///
/// Newtype rather than `String` so it cannot be confused with the
/// human-readable root at a call site — which is the confusion B3 is about.
/// **Never** `global`, never a path.
/// # Why this deliberately does NOT derive `Deserialize`
///
/// It used to, and that quietly falsified the safety argument stated on
/// `Vendor::hook_registration`:
/// serde gives a private-field newtype a **transparent** deserializer, so
/// `serde_json::from_str::<RootToken>("\"anything\"")` minted one — making the
/// type exactly as forgeable from a `&str` as the absolute path it replaced,
/// which is the property its own existence is meant to deny. Found by the WP-J1
/// author while looking for a legitimate way to build a probe token.
///
/// The dispatch table still has to round-trip its keys, so the capability lives
/// in the one reader that needs it ([`read_table`]) rather than on the type,
/// where any caller could reach it. `Serialize` stays: writing a token is not
/// minting one.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct RootToken(String);

impl RootToken {
    /// The token's wire form — 32 lowercase hex characters.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Accepts exactly 32 lowercase hex characters (`[0-9a-f]{32}`) and nothing else.
    ///
    /// The one public constructor: only the keyed mint in `grim` calls it, after
    /// hex-encoding a truncated MAC, so a token still cannot be conjured from an
    /// arbitrary `&str`. [`read_table`] reconstitutes stored keys through a private
    /// unchecked constructor instead, so a hand-edited non-hex root is read exactly
    /// as it always was.
    pub fn parse(value: &str) -> Option<Self> {
        let well_formed = value.len() == 32 && value.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'));
        well_formed.then(|| Self(value.to_owned()))
    }

    /// A token with a caller-chosen value, **tests only**.
    ///
    /// Behind the `test-util` feature, so it does not exist in the shipped binary and cannot
    /// widen the type for production callers. It exists because a byte-exact
    /// assertion on a generated command string needs a known token, while
    /// `root_token` derives one from a per-`$GRIM_HOME` random key.
    ///
    /// Before this existed, a test helper minted tokens with
    /// `serde_json::from_value` — the very transparent-newtype route that
    /// falsified the type's safety claim. Removing `Deserialize` broke that
    /// helper, which is how the hole was closed rather than merely documented.
    #[cfg(feature = "test-util")]
    pub fn for_test(value: &str) -> Self {
        Self(value.to_owned())
    }
}

impl RootToken {
    /// The reader's private way in: wraps a stored key unvalidated, as the table has
    /// always been read.
    fn from_stored(value: String) -> Self {
        Self(value)
    }
}

impl std::fmt::Display for RootToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// One armed hook, as the runtime needs it — the dispatch table's leaf.
///
/// Everything here is grim-resolved at install time. The runtime looks a row up
/// by `(root token, client, event)` plus its own matcher pass and spawns
/// `handler` from `payload_dir`; it **hashes nothing** (C-009).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "serde_json::Value")]
pub struct DispatchEntry {
    /// The config binding name of the artifact this entry came from. Reaches
    /// the audit trail as `<artifact>/<id>`; never a shell string (C-018b).
    pub artifact: String,
    /// `crate::oci::hook::HookEntry::id`, unique within the artifact.
    pub id: String,
    /// **The client this row is armed for** — the second component of the
    /// `(root token, client, event)` selection key above, and the reason a row
    /// exists per `(hook, client)` rather than per hook.
    ///
    /// # Why this field is required, and why that is the *safe* choice
    ///
    /// Without it the dimension is not merely unpopulated, it is
    /// **unrecoverable**. Every other field on this struct is client-*independent*:
    /// `artifact`, `id`, `event`, `tier`, `matcher`, `handler`, `timeout`,
    /// `payload` and `policy` come from the record and the manifest, and
    /// `payload_dir` is one directory per scope shared by every arming client
    /// (S-003). So the two shapes the convergence loop bridges cannot be joined:
    /// `super::hook_registrar::desired_entries` is **per vendor** (it filters
    /// `record.outputs` by `o.client`), while `converge_root` replaces a root's
    /// whole `hooks` vector **wholesale**. Calling it once per vendor makes each
    /// vendor's write wipe the previous vendor's rows; unioning across vendors
    /// instead yields *N byte-identical rows*, which is worse than a duplicate —
    /// it means a hook grim `Declined` for one client (an untranslatable matcher
    /// per C-025, a tier that client cannot honour) is indistinguishable from one
    /// it armed there, so **the declining client runs code the user was told was
    /// not armed for it.** That converts a render-time decline into a silent
    /// arming, which is the failure C-017 and C-025 exist to prevent.
    ///
    /// # Required, not `#[serde(default)]` — Principle 9 does not apply yet
    ///
    /// Principle 9 governs **released** surfaces, and this table has never
    /// shipped: `git ls-tree 03e59b0 -- src/install/` (the v0.13.0 release
    /// commit) contains **no hook file at all**, and hooks are gated off behind
    /// `[options.experimental]` regardless. There is no `dispatch.json` anywhere
    /// to be compatible with, so this is the one free moment to make the field
    /// mandatory. A defaulted `String` would yield `""` for a client-less row,
    /// and `""` would then have to mean either "matches nothing" or "matches
    /// every client" — an ambiguity sitting in exactly the row-selection path
    /// that decides whether a declining client executes code. Required makes
    /// that row unrepresentable, and W2's row-reject-with-a-log-line already
    /// makes a malformed row degrade to *not armed* rather than to *armed for
    /// everyone*. [`DISPATCH_SCHEMA`] therefore stays `1`.
    ///
    /// A `String` and not `ClientTarget`:
    /// this is the same spelling `ClientOutput::client`
    /// carries, so the two structures the convergence loop bridges name a client
    /// one way, and a legacy or unparsable client name recorded in `state.json`
    /// stays representable here — it simply selects no row at dispatch time,
    /// which is the fail-safe direction.
    pub client: String,
    /// The canonical firing event.
    pub event: CanonicalEvent,
    /// What the handler may do with the moment.
    pub tier: HookTier,
    /// grim's **own** matcher dialect, kept verbatim so grim — not the vendor —
    /// owns matching (`None` = every tool). Re-checked against
    /// `MATCHER_MAX_BYTES` at read time (W2 (c)): the build-time cap does not
    /// bind a file on disk.
    pub matcher: Option<String>,
    /// The program to run.
    pub handler: HookHandler,
    /// Grim-enforced timeout in seconds; `None` = the format default.
    pub timeout: Option<u64>,
    /// Envelope transport.
    pub payload: HookPayloadMode,
    /// The materialized payload tree the handler runs from — absolute, resolved
    /// from the recorded `ClientOutput` at convergence time.
    pub payload_dir: PathBuf,
    /// The digest the artifact resolved to at lock time — **provenance for
    /// diagnostics only, never a gate** (W4 · I5).
    ///
    /// It was `approved digest` and that name was a false security claim: A2
    /// deleted the approval store, A3 deleted the exec-time re-check, and the
    /// runtime hashes nothing (C-009). A field named `approved` that gates
    /// nothing gets read as a control by the next reviewer. `None` for a
    /// path-sourced (dev) install, which has no registry pin.
    pub resolved_digest: Option<String>,
    /// The reserved `policy` key, carried through unparsed so a future
    /// vocabulary lands additively.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub policy: Option<serde_json::Value>,
}

/// The field a row failed on; names it and nothing else, so the launcher links
/// no per-field message.
#[derive(Debug)]
pub struct BadField(&'static str);

impl std::fmt::Display for BadField {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "field `{}` is missing or has the wrong type", self.0)
    }
}

/// Extracts a row by hand instead of through serde's derive: the derive is the
/// largest single piece of the launcher's read path, and this is the one reader
/// for both the launcher and `grim` (`Deserialize` delegates here through
/// `try_from`). Matches the derive it replaced: unknown members are ignored,
/// `null` and absent both read as `None`, a required member of the wrong type
/// fails the row.
impl TryFrom<serde_json::Value> for DispatchEntry {
    type Error = BadField;

    fn try_from(value: serde_json::Value) -> Result<Self, BadField> {
        use serde_json::Value;

        let Value::Object(mut row) = value else {
            return Err(BadField("row"));
        };
        fn text(row: &mut serde_json::Map<String, Value>, key: &'static str) -> Result<String, BadField> {
            match row.remove(key) {
                Some(Value::String(s)) => Ok(s),
                _ => Err(BadField(key)),
            }
        }
        fn opt_text(row: &mut serde_json::Map<String, Value>, key: &'static str) -> Result<Option<String>, BadField> {
            match row.remove(key) {
                None | Some(Value::Null) => Ok(None),
                Some(Value::String(s)) => Ok(Some(s)),
                Some(_) => Err(BadField(key)),
            }
        }
        let tier = text(&mut row, "tier")?;
        let timeout = match row.remove("timeout") {
            None | Some(Value::Null) => None,
            Some(Value::Number(n)) => Some(n.as_u64().ok_or(BadField("timeout"))?),
            Some(_) => return Err(BadField("timeout")),
        };
        Ok(Self {
            artifact: text(&mut row, "artifact")?,
            id: text(&mut row, "id")?,
            client: text(&mut row, "client")?,
            event: canonical_event(&text(&mut row, "event")?).ok_or(BadField("event"))?,
            tier: HookTier::ALL
                .into_iter()
                .find(|t| t.as_str() == tier)
                .ok_or(BadField("tier"))?,
            matcher: opt_text(&mut row, "matcher")?,
            handler: match row.remove("handler") {
                // Unknown keys are ignored, like everywhere else in a row (additive
                // evolution); exactly one of the two known forms must be present.
                Some(Value::Object(mut h)) => match (h.remove("command"), h.remove("argv")) {
                    (Some(Value::String(command)), None) => HookHandler::Command(command),
                    (None, Some(Value::Array(items))) => HookHandler::Argv(
                        items
                            .into_iter()
                            .map(|item| match item {
                                Value::String(s) => Ok(s),
                                _ => Err(BadField("handler")),
                            })
                            .collect::<Result<_, _>>()?,
                    ),
                    _ => return Err(BadField("handler")),
                },
                _ => return Err(BadField("handler")),
            },
            timeout,
            payload: match text(&mut row, "payload")?.as_str() {
                "stdin" => HookPayloadMode::Stdin,
                "file" => HookPayloadMode::File,
                _ => return Err(BadField("payload")),
            },
            payload_dir: PathBuf::from(text(&mut row, "payload_dir")?),
            resolved_digest: opt_text(&mut row, "resolved_digest")?,
            policy: row.remove("policy").filter(|p| !p.is_null()),
        })
    }
}

/// A JSON document read with duplicate object keys refused.
///
/// `serde_json::Value` keeps the last of two equal keys, so a hand-edited table
/// or a hook's answer carrying `"decision":"deny"` twice would read as whichever
/// came last. Refusing the document makes both readers see one meaning or none.
pub(crate) struct StrictValue(pub(crate) serde_json::Value);

impl<'de> Deserialize<'de> for StrictValue {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(StrictVisitor)
    }
}

struct StrictVisitor;

impl<'de> serde::de::Visitor<'de> for StrictVisitor {
    type Value = StrictValue;

    fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("a JSON value")
    }

    fn visit_bool<E>(self, v: bool) -> Result<StrictValue, E> {
        Ok(StrictValue(serde_json::Value::Bool(v)))
    }

    fn visit_i64<E>(self, v: i64) -> Result<StrictValue, E> {
        Ok(StrictValue(v.into()))
    }

    fn visit_u64<E>(self, v: u64) -> Result<StrictValue, E> {
        Ok(StrictValue(v.into()))
    }

    fn visit_f64<E: serde::de::Error>(self, v: f64) -> Result<StrictValue, E> {
        serde_json::Number::from_f64(v)
            .map(|n| StrictValue(serde_json::Value::Number(n)))
            .ok_or_else(|| E::custom("a non-finite number"))
    }

    fn visit_str<E>(self, v: &str) -> Result<StrictValue, E> {
        Ok(StrictValue(serde_json::Value::String(v.to_owned())))
    }

    fn visit_string<E>(self, v: String) -> Result<StrictValue, E> {
        Ok(StrictValue(serde_json::Value::String(v)))
    }

    fn visit_unit<E>(self) -> Result<StrictValue, E> {
        Ok(StrictValue(serde_json::Value::Null))
    }

    fn visit_seq<A: serde::de::SeqAccess<'de>>(self, mut seq: A) -> Result<StrictValue, A::Error> {
        let mut items = Vec::new();
        while let Some(StrictValue(item)) = seq.next_element()? {
            items.push(item);
        }
        Ok(StrictValue(serde_json::Value::Array(items)))
    }

    fn visit_map<A: serde::de::MapAccess<'de>>(self, mut map: A) -> Result<StrictValue, A::Error> {
        let mut members = serde_json::Map::new();
        while let Some(key) = map.next_key::<String>()? {
            let StrictValue(value) = map.next_value()?;
            if members.contains_key(&key) {
                return Err(serde::de::Error::custom(format!("duplicate key `{key}`")));
            }
            members.insert(key, value);
        }
        Ok(StrictValue(serde_json::Value::Object(members)))
    }
}

/// The one reading of a table's bytes into an untyped document, duplicate keys refused.
fn parse_document(bytes: &[u8]) -> Option<serde_json::Value> {
    serde_json::from_slice::<StrictValue>(bytes).ok().map(|v| v.0)
}

/// Why [`accept_row`] refused a row.
enum RowFault {
    /// The row does not read as a [`DispatchEntry`].
    Unreadable(BadField),
    /// The row reads, but fails a read-time re-check a build-time cap cannot bind.
    Recheck(String),
}

impl std::fmt::Display for RowFault {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unreadable(e) => e.fmt(f),
            Self::Recheck(why) => f.write_str(why),
        }
    }
}

/// The one row-acceptance rule, shared by [`parse_table`] and [`select`] so
/// `grim status` and the launcher cannot disagree about which rows are armed:
/// the row must read as a [`DispatchEntry`], its matcher must fit
/// `MATCHER_MAX_BYTES` and its `payload_dir` must be absolute.
fn accept_row(row: serde_json::Value) -> Result<DispatchEntry, RowFault> {
    let entry = DispatchEntry::try_from(row).map_err(RowFault::Unreadable)?;
    if entry.matcher.as_ref().is_some_and(|m| m.len() > MATCHER_MAX_BYTES) {
        return Err(RowFault::Recheck(format!(
            "{}/{}: matcher exceeds {MATCHER_MAX_BYTES} bytes",
            entry.artifact, entry.id
        )));
    }
    if !entry.payload_dir.is_absolute() {
        return Err(RowFault::Recheck(format!(
            "{}/{}: payload_dir is not absolute",
            entry.artifact, entry.id
        )));
    }
    Ok(entry)
}

/// Every hook armed for one root, plus that root in readable form.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DispatchRoot {
    /// `RootScope::display` — **diagnostics only.** Never matched against
    /// anything at runtime, never compared to the invoking workspace (C-007),
    /// and never derived back into a token.
    pub root: String,
    /// The armed hooks, in a stable order so the written bytes are
    /// deterministic.
    pub hooks: Vec<DispatchEntry>,
}

/// The whole table: `schema` plus one entry per root token.
///
/// One file for the whole machine — but see the module doc: "machine-local" is
/// a property of *where grim writes it*, made true at runtime only by the
/// argv-supplied absolute path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DispatchTable {
    /// Read first, before any other field (W2 (a)).
    pub schema: u32,
    /// Armed hooks per opaque root token, holding only the rows this binary can
    /// read. No `Deserialize`: [`read_table`] builds it row by row, and is the
    /// only place a [`RootToken`] is reconstituted from stored bytes.
    pub roots: BTreeMap<RootToken, DispatchRoot>,
}

/// One root as stored, its rows still untyped so each is read on its own.
#[derive(Deserialize)]
struct StoredRoot {
    root: String,
    hooks: Vec<serde_json::Value>,
}

impl DispatchTable {
    /// The empty table at the current schema — also what every W2 degrade
    /// resolves to, so "unreadable" and "nothing armed" are one code path.
    pub fn empty() -> Self {
        Self {
            schema: DISPATCH_SCHEMA,
            roots: BTreeMap::new(),
        }
    }
}

/// Why a table read produced [`DispatchTable::empty`] instead of content —
/// one log line's worth of reason, never an error (W2 · I3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DispatchDegrade {
    /// No table on disk — the ordinary "no hooks armed" state, including the
    /// feature being off (Decision N: "off" reaches the runtime structurally,
    /// as an empty table).
    Absent,
    /// Larger than [`MAX_TABLE_BYTES`].
    Oversize,
    /// Not readable as JSON, not an object, or without a `roots` object.
    Unparsable,
    /// A `schema` this binary does not recognize — **including a newer one**
    /// after a downgrade. Never an error: a downgraded grim must degrade to
    /// "hooks off", not block the agent.
    UnknownSchema,
    /// A row failed the read-time re-check (`MATCHER_MAX_BYTES`, a
    /// non-absolute `payload_dir`).
    ///
    /// Whole-table, not per-row: a table grim cannot fully vouch for arms
    /// nothing, because "some rows survived" is how a tampered table gets a
    /// partial verdict honoured.
    RowRejected,
}

/// Read the table at `path` — **never fails, never panics**.
///
/// File half of [`parse_table`], with the size cap applied before the read.
/// Skipped rows are dropped silently; use [`read_table_diagnosed`] to see why.
pub fn read_table(path: &Path) -> (DispatchTable, Option<DispatchDegrade>) {
    read_table_diagnosed(path, &mut Vec::new())
}

/// [`read_table`], with the skipped-row diagnostics handed to the caller as
/// [`parse_table`] hands them.
///
/// `path` is the table file as the caller resolved it; the launcher never goes
/// through here, it finds `dispatch.json` beside its own executable.
pub fn read_table_diagnosed(
    path: &Path,
    diagnostics: &mut Vec<Diagnostic>,
) -> (DispatchTable, Option<DispatchDegrade>) {
    match read_table_bytes(path) {
        Ok(bytes) => parse_table(&bytes, diagnostics),
        Err(reason) => (DispatchTable::empty(), Some(reason)),
    }
}

/// The table file's bytes, size-capped before the read — a cap applied after
/// reading the file is not a cap.
pub fn read_table_bytes(path: &Path) -> Result<Vec<u8>, DispatchDegrade> {
    match std::fs::metadata(path) {
        Ok(meta) if meta.len() > MAX_TABLE_BYTES => return Err(DispatchDegrade::Oversize),
        Ok(_) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Err(DispatchDegrade::Absent),
        // Unreadable for any other reason is not "absent": the file may well be
        // there. It is still the same degrade direction (I3) but a different
        // log line, so it is reported as unparsable rather than as absent.
        Err(_) => return Err(DispatchDegrade::Unparsable),
    }
    match std::fs::read(path) {
        Ok(bytes) => Ok(bytes),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Err(DispatchDegrade::Absent),
        Err(_) => Err(DispatchDegrade::Unparsable),
    }
}

/// Parse the table's `bytes` — **never fails, never panics**.
///
/// The order the checks run: size cap, parse (duplicate keys refused), `schema`,
/// then each row through the same rule [`select`] applies. A failure yields
/// [`DispatchTable::empty`] plus the [`DispatchDegrade`] reason.
///
/// **One row rule** (`accept_row`) serves this reader and the launcher's
/// [`select`], so `grim status` and a firing hook agree on which rows are armed.
/// They differ only in the consequence of a refusal. A row that does not read is
/// skipped with a debug diagnostic here, as `select` skips it when it is not
/// addressed to the invocation. A row that reads but fails the matcher-length or
/// absolute-`payload_dir` re-check degrades the whole table here
/// ([`DispatchDegrade::RowRejected`], so status reports nothing armed), while
/// `select` fails only the invocation that selects it.
pub fn parse_table(bytes: &[u8], diagnostics: &mut Vec<Diagnostic>) -> (DispatchTable, Option<DispatchDegrade>) {
    let degraded = |reason: DispatchDegrade| (DispatchTable::empty(), Some(reason));
    let mut stored = match parse_stored(bytes) {
        Ok(stored) => stored,
        Err(reason) => return degraded(reason),
    };

    // Row by row: every grim on the machine shares this file, so a row an older
    // binary cannot read (a newer tier, event or handler) must cost only that
    // row, never every other workspace's hooks.
    let mut table = DispatchTable::empty();
    let mut rechecks_failed = false;
    if let Some(serde_json::Value::Object(roots)) = stored.remove("roots") {
        for (token, root) in roots {
            let Ok(root) = serde_json::from_value::<StoredRoot>(root) else {
                diagnostics.push(Diagnostic::debug("dispatch root this grim cannot read; skipped"));
                continue;
            };
            let hooks = root
                .hooks
                .into_iter()
                .filter_map(|row| match accept_row(row) {
                    Ok(entry) => Some(entry),
                    Err(fault) => {
                        rechecks_failed |= matches!(fault, RowFault::Recheck(_));
                        diagnostics.push(Diagnostic::debug(format!(
                            "dispatch row this grim cannot accept; skipped: {fault}"
                        )));
                        None
                    }
                })
                .collect();
            table
                .roots
                .insert(RootToken::from_stored(token), DispatchRoot { root: root.root, hooks });
        }
    }
    // A table grim cannot fully vouch for arms nothing: "some rows survived" is
    // how a tampered table gets a partial verdict honoured.
    if rechecks_failed {
        return degraded(DispatchDegrade::RowRejected);
    }

    (table, None)
}

/// The table's top-level object exactly as stored, every key kept and no row
/// typed yet — the shared base of [`read_table`] and `converge_root`.
///
/// `Ok` guarantees a `schema` this binary understands and a `roots` object.
pub fn read_stored(path: &Path) -> Result<serde_json::Map<String, serde_json::Value>, DispatchDegrade> {
    parse_stored(&read_table_bytes(path)?)
}

/// [`read_stored`] over bytes already in hand.
pub fn parse_stored(bytes: &[u8]) -> Result<serde_json::Map<String, serde_json::Value>, DispatchDegrade> {
    // (c) The size cap, again for bytes that did not come through
    // `read_table_bytes`.
    if bytes.len() as u64 > MAX_TABLE_BYTES {
        return Err(DispatchDegrade::Oversize);
    }
    // (a) `schema` first, through an untyped parse, so a newer schema reports as
    // `UnknownSchema` rather than as whatever its row shape fails on.
    let Some(serde_json::Value::Object(object)) = parse_document(bytes) else {
        return Err(DispatchDegrade::Unparsable);
    };
    // (b) Any unrecognized value — including a newer one after a downgrade.
    if object.get("schema").and_then(serde_json::Value::as_u64) != Some(u64::from(DISPATCH_SCHEMA)) {
        return Err(DispatchDegrade::UnknownSchema);
    }
    if !object.get("roots").is_some_and(serde_json::Value::is_object) {
        return Err(DispatchDegrade::Unparsable);
    }
    Ok(object)
}

/// What [`select`] picked for one invocation.
#[derive(Debug, Clone, PartialEq)]
pub struct Selection {
    /// Whether the table has a root for the invocation's token. `false` is the
    /// benign unknown-root case: the launcher warns under `GRIM_LOG` and exits 0.
    pub root_known: bool,
    /// The rows addressed to `(client, canonical event)`, fully parsed, in stored
    /// order. The tool matcher has not been applied.
    pub rows: Vec<DispatchEntry>,
    /// The root's readable path (`DispatchRoot::root`), diagnostics only; empty when
    /// the root is unknown or unreadable.
    pub scope: String,
    /// Rows or a root skipped as unreadable or not addressed here, as debug lines.
    pub diagnostics: Vec<Diagnostic>,
}

/// Why [`select`] could not answer. Every variant maps to exit 65.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum TableError {
    /// Larger than [`MAX_TABLE_BYTES`].
    TooLarge,
    /// Not JSON, or not an object.
    Unparsable,
    /// `schema` is absent or not a non-negative integer, or is newer than
    /// [`DISPATCH_SCHEMA`].
    Schema(SchemaFault),
    /// `roots` is absent or not an object.
    RootsNotObject,
    /// A row addressed to this invocation failed to parse, or failed the
    /// read-time re-checks (`MATCHER_MAX_BYTES`, an absolute `payload_dir`).
    /// Skipping it could silently drop a gatekeeper.
    Row(String),
}

/// How a table's `schema` was unacceptable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SchemaFault {
    /// Absent, or not a non-negative integer.
    Missing,
    /// Newer than the schema this build knows.
    Newer(u64),
}

impl TableError {
    /// The launcher's exit status for this error.
    pub fn exit(&self) -> crate::exit::DispatchExit {
        crate::exit::DispatchExit::DataError
    }
}

impl std::fmt::Display for TableError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TooLarge => write!(f, "dispatch table exceeds {MAX_TABLE_BYTES} bytes"),
            Self::Unparsable => f.write_str("dispatch table is not a JSON object"),
            Self::Schema(SchemaFault::Missing) => f.write_str("dispatch table has no integer schema"),
            Self::Schema(SchemaFault::Newer(n)) => {
                write!(f, "dispatch table schema {n} is newer than {DISPATCH_SCHEMA}")
            }
            Self::RootsNotObject => f.write_str("dispatch table roots is not an object"),
            Self::Row(why) => write!(f, "a selected dispatch row is unreadable: {why}"),
        }
    }
}

impl std::error::Error for TableError {}

/// The launcher's fire-time reader: the rows armed for one invocation.
///
/// `parse_table` serves the writer, `status` and `install`; this is the only
/// reader on the hot path, and both apply one row rule (see [`parse_table`]).
/// In order: the size cap, then the top level as a plain `serde_json::Value`
/// (no derive and no `RawValue`: both are a measurable share of the launcher's
/// size budget, and a table is a few kilobytes), then `schema` (exactly
/// up to [`DISPATCH_SCHEMA`]; a newer one is an error). An unknown root is an empty
/// selection, not an error, and so is an event that names no canonical event or
/// a client with no projection at that event.
///
/// Each row is probed for string `client` and `event`; a failed probe or a
/// mismatch against `(inv.client, canonical event)` skips the row. A row that
/// passes the probe must then parse fully or the call errors, because skipping it
/// could drop a gatekeeper. A root this build cannot read at all is skipped, per
/// the additive-read rule. **The tool matcher is not evaluated here**; dispatch
/// applies it last, over the returned rows.
///
/// # Errors
///
/// [`TableError`] when the table itself is unusable or a selected row is unreadable.
pub fn select(bytes: &[u8], inv: &crate::argv::Invocation) -> Result<Selection, TableError> {
    use serde_json::Value;

    // The size cap first: a cap applied after parsing is not a cap.
    if bytes.len() as u64 > MAX_TABLE_BYTES {
        return Err(TableError::TooLarge);
    }
    let Some(Value::Object(mut top)) = parse_document(bytes) else {
        return Err(TableError::Unparsable);
    };
    let schema = top
        .remove("schema")
        .and_then(|v| v.as_u64())
        .ok_or(TableError::Schema(SchemaFault::Missing))?;
    if schema > u64::from(DISPATCH_SCHEMA) {
        return Err(TableError::Schema(SchemaFault::Newer(schema)));
    }
    let Some(Value::Object(mut roots)) = top.remove("roots") else {
        return Err(TableError::RootsNotObject);
    };

    let mut selection = Selection {
        root_known: false,
        rows: Vec::new(),
        scope: String::new(),
        diagnostics: Vec::new(),
    };
    let Some(root) = roots.remove(inv.root.as_str()) else {
        selection
            .diagnostics
            .push(Diagnostic::debug("no hooks armed for the requested root; nothing ran"));
        return Ok(selection);
    };
    selection.root_known = true;

    let (scope, hooks) = match root {
        Value::Object(mut root) => (root.remove("root"), root.remove("hooks")),
        _ => (None, None),
    };
    let (Some(Value::String(scope)), Some(Value::Array(hooks))) = (scope, hooks) else {
        selection
            .diagnostics
            .push(Diagnostic::debug("dispatch root this build cannot read; skipped"));
        return Ok(selection);
    };
    selection.scope = scope;
    // An event naming no canonical event addresses no row, and is not an error.
    let Some(event) = crate::canonical_event(&inv.event) else {
        selection
            .diagnostics
            .push(Diagnostic::debug("event maps to no canonical event; nothing ran"));
        return Ok(selection);
    };

    // A client with no projection at this event is one this build cannot answer
    // for (a newer client literal, say). Running its gatekeepers only to drop
    // their verdict would forge a pass, so nothing is selected.
    if crate::projection_for(&inv.client, event).is_none() {
        selection.diagnostics.push(Diagnostic::debug(
            "no projection for this client and event; nothing ran",
        ));
        return Ok(selection);
    }

    for row in hooks {
        let addressed = row.get("client").and_then(Value::as_str) == Some(inv.client.as_str())
            && row.get("event").and_then(Value::as_str) == Some(event.as_str());
        if !addressed {
            continue;
        }
        // Skipping a selected row could silently drop a gatekeeper, so it is an error.
        let entry = accept_row(row).map_err(|e| TableError::Row(e.to_string()))?;
        selection.rows.push(entry);
    }
    Ok(selection)
}

#[cfg(test)]
mod select_contract;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_accepts_exactly_32_lowercase_hex() {
        let ok = "0123456789abcdef0123456789abcdef";
        assert_eq!(RootToken::parse(ok).as_ref().map(RootToken::as_str), Some(ok));
        for bad in [
            "0123456789ABCDEF0123456789ABCDEF",
            "0123456789abcdef0123456789abcde",
            "0123456789abcdef0123456789abcdef0",
            "0123456789abcdef0123456789abcdeg",
            "",
        ] {
            assert!(RootToken::parse(bad).is_none(), "{bad:?} must not parse");
        }
    }

    #[test]
    fn read_table_keeps_a_stored_root_whose_key_is_not_hex() -> io::Result<()> {
        let dir = std::env::temp_dir().join(format!("hook-core-table-{}", std::process::id()));
        std::fs::create_dir_all(&dir)?;
        let path = dir.join("dispatch.json");
        std::fs::write(&path, r#"{"schema":1,"roots":{"NOT-HEX":{"root":"r","hooks":[]}}}"#)?;
        let (table, degrade) = read_table(&path);
        std::fs::remove_dir_all(&dir)?;
        assert!(degrade.is_none());
        assert_eq!(
            table.roots.keys().map(RootToken::as_str).collect::<Vec<_>>(),
            ["NOT-HEX"]
        );
        Ok(())
    }
}
