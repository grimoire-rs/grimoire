// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! `c/root.json`, the small mutable file naming an index build's immutable
//! `c/f/` blobs. A root this module rejects is a miss, never an error: the
//! refresh falls back to `/all.json`.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::index_http::INDEX_BODY_LIMIT;

/// The most bytes a root may have; a larger answer is a miss.
pub const ROOT_LIMIT: usize = 64 * 1024;

/// Inflated size a flat or stats blob may declare, the decoded-body ceiling.
const BLOB_SIZE_LIMIT: u64 = INDEX_BODY_LIMIT as u64;

/// The largest integer a JSON double holds exactly, the most a producer emits.
const MAX_GENERATION: u64 = (1 << 53) - 1;

/// A content-addressed blob: the SHA-256 (lowercase hex) and size of its
/// inflated bytes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlobRef {
    pub sha256: String,
    pub size: u64,
}

/// The typed part of a `schema: 1` root. `planes`, `search` and every
/// unknown member are checked for shape at most, never typed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Root {
    /// Informational; logged on adoption, never compared.
    pub generation: u64,
    /// The flat blob, or `None` for a root that names none (`layout: flat`).
    pub flat: Option<BlobRef>,
    /// The stats blob, or `None` when the build has no stats.
    pub stats: Option<BlobRef>,
}

/// Why a 200 answer is not a usable root.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RootMiss {
    #[error("root is larger than {ROOT_LIMIT} bytes")]
    TooLarge,
    #[error("root is not a JSON object")]
    NotJson,
    #[error("root schema is not 1")]
    UnsupportedSchema,
    #[error("root field `{0}` is invalid")]
    Invalid(&'static str),
}

/// Parses and validates a root body. Every digest passes `^[0-9a-f]{64}$`
/// and every size `0 < size ≤ cap` before any URL is built from it.
pub fn parse_root(bytes: &[u8]) -> Result<Root, RootMiss> {
    if bytes.len() > ROOT_LIMIT {
        return Err(RootMiss::TooLarge);
    }
    let Ok(Value::Object(root)) = serde_json::from_slice(bytes) else {
        return Err(RootMiss::NotJson);
    };
    // An integer only: `1.0` and `"1"` are other documents.
    if root.get("schema").and_then(Value::as_u64) != Some(1) {
        return Err(RootMiss::UnsupportedSchema);
    }
    // Members of `planes` stay untyped, so only the container is checked.
    if !root.get("planes").is_some_and(Value::is_object) {
        return Err(RootMiss::Invalid("planes"));
    }
    Ok(Root {
        // Never compared, so a value out of range costs nothing worth a miss.
        generation: root
            .get("generation")
            .and_then(Value::as_u64)
            .filter(|generation| *generation <= MAX_GENERATION)
            .unwrap_or(0),
        flat: blob_ref(&root, "flat")?,
        stats: blob_ref(&root, "stats")?,
    })
}

/// Whether `digest` is 64 lowercase hex digits.
pub(crate) fn is_digest(digest: &str) -> bool {
    digest.len() == 64 && digest.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// A member that is `null` or a ref; absent is as wrong as malformed.
fn blob_ref(root: &Map<String, Value>, field: &'static str) -> Result<Option<BlobRef>, RootMiss> {
    match root.get(field) {
        Some(Value::Null) => Ok(None),
        Some(Value::Object(blob)) => {
            let sha256 = blob
                .get("sha256")
                .and_then(Value::as_str)
                .filter(|digest| is_digest(digest))
                .ok_or(RootMiss::Invalid(field))?;
            let size = blob
                .get("size")
                .and_then(Value::as_u64)
                .filter(|size| (1..=BLOB_SIZE_LIMIT).contains(size))
                .ok_or(RootMiss::Invalid(field))?;
            Ok(Some(BlobRef {
                sha256: sha256.to_owned(),
                size,
            }))
        }
        _ => Err(RootMiss::Invalid(field)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DIGEST: &str = "9f2c0000000000000000000000000000000000000000000000000000000000aa";

    fn ref_json(digest: &str, size: &str) -> String {
        format!(r#"{{"sha256":"{digest}","size":{size}}}"#)
    }

    fn root_with(flat: &str, stats: &str, planes: &str) -> Vec<u8> {
        format!(
            r#"{{"flat":{flat},"generated_at":"2026-10-21T08:00:00Z","generation":1,"planes":{planes},"schema":1,"search":null,"stats":{stats}}}"#
        )
        .into_bytes()
    }

    fn root_json(flat: &str) -> Vec<u8> {
        root_with(flat, "null", r#"{"catalog":null,"enrich":"x"}"#)
    }

    fn flat_ref() -> String {
        ref_json(DIGEST, "10")
    }

    #[test]
    fn parse_root_accepts_untyped_planes() {
        let root = parse_root(&root_json(&flat_ref())).unwrap();
        assert_eq!(
            root.flat,
            Some(BlobRef {
                sha256: DIGEST.into(),
                size: 10
            })
        );
        assert_eq!(root.stats, None);
    }

    #[test]
    fn parse_root_rejects_an_uppercase_digest() {
        let flat = ref_json(&DIGEST.to_uppercase(), "10");
        assert_eq!(parse_root(&root_json(&flat)), Err(RootMiss::Invalid("flat")));
    }

    #[test]
    fn parse_root_treats_schema_2_as_a_miss() {
        let body = br#"{"flat":null,"generation":1,"planes":{},"schema":2,"search":null,"stats":null}"#;
        assert_eq!(parse_root(body), Err(RootMiss::UnsupportedSchema));
    }

    #[test]
    fn parse_root_requires_schema_to_be_the_integer_1() {
        for schema in [r#""1""#, "1.0", "true", "null", "[1]", "0"] {
            let body =
                format!(r#"{{"flat":null,"generation":1,"planes":{{}},"schema":{schema},"search":null,"stats":null}}"#);
            assert_eq!(
                parse_root(body.as_bytes()),
                Err(RootMiss::UnsupportedSchema),
                "schema {schema}"
            );
        }
        let absent = br#"{"flat":null,"generation":1,"planes":{},"search":null,"stats":null}"#;
        assert_eq!(parse_root(absent), Err(RootMiss::UnsupportedSchema));
    }

    #[test]
    fn parse_root_rejects_a_body_over_the_cap_but_not_one_at_it() {
        let pad = |len: usize| {
            let head = br#"{"flat":null,"generation":1,"planes":{},"schema":1,"search":null,"stats":null,"x":""#;
            let tail = br#""}"#;
            let mut body = head.to_vec();
            body.resize(len - tail.len(), b'a');
            body.extend_from_slice(tail);
            body
        };
        assert!(parse_root(&pad(ROOT_LIMIT)).is_ok());
        assert_eq!(parse_root(&pad(ROOT_LIMIT + 1)), Err(RootMiss::TooLarge));
    }

    #[test]
    fn parse_root_rejects_what_is_not_a_json_object() {
        for body in ["", "<html>not found</html>", "[]", "null", "1", r#""root""#, "{"] {
            assert_eq!(parse_root(body.as_bytes()), Err(RootMiss::NotJson), "{body:?}");
        }
    }

    #[test]
    fn parse_root_accepts_a_null_flat_and_a_null_stats() {
        let root = parse_root(&root_with("null", "null", "{}")).unwrap();
        assert_eq!((root.flat, root.stats), (None, None));
    }

    #[test]
    fn parse_root_reads_the_stats_ref() {
        let stats = ref_json(DIGEST, "3000000");
        let root = parse_root(&root_with("null", &stats, "{}")).unwrap();
        assert_eq!(
            root.stats,
            Some(BlobRef {
                sha256: DIGEST.into(),
                size: 3_000_000
            })
        );
    }

    #[test]
    fn parse_root_rejects_a_malformed_ref_in_either_slot() {
        let cap = INDEX_BODY_LIMIT as u64;
        let over = (cap + 1).to_string();
        let at = cap.to_string();
        let refs = [
            ref_json(&DIGEST[..63], "10"),
            ref_json(&format!("{DIGEST}0"), "10"),
            ref_json(&DIGEST.replace('a', "g"), "10"),
            ref_json(DIGEST, "0"),
            ref_json(DIGEST, "-1"),
            ref_json(DIGEST, "1.5"),
            ref_json(DIGEST, r#""10""#),
            ref_json(DIGEST, "null"),
            ref_json(DIGEST, &over),
            r#"{"sha256":"9f2c"}"#.into(),
            "{}".into(),
            r#""blob""#.into(),
            "[]".into(),
            "10".into(),
            "true".into(),
        ];
        for bad in &refs {
            assert_eq!(
                parse_root(&root_with(bad, "null", "{}")),
                Err(RootMiss::Invalid("flat")),
                "flat {bad}"
            );
            assert_eq!(
                parse_root(&root_with("null", bad, "{}")),
                Err(RootMiss::Invalid("stats")),
                "stats {bad}"
            );
        }
        let max = ref_json(DIGEST, &at);
        assert!(parse_root(&root_with(&max, &max, "{}")).is_ok());
    }

    #[test]
    fn parse_root_ignores_unknown_members_of_a_ref() {
        let flat = format!(r#"{{"extra":[1],"sha256":"{DIGEST}","size":10}}"#);
        assert!(parse_root(&root_json(&flat)).unwrap().flat.is_some());
    }

    #[test]
    fn parse_root_requires_flat_stats_and_planes_to_be_present() {
        let no_flat = br#"{"generation":1,"planes":{},"schema":1,"search":null,"stats":null}"#;
        let no_stats = br#"{"flat":null,"generation":1,"planes":{},"schema":1,"search":null}"#;
        let no_planes = br#"{"flat":null,"generation":1,"schema":1,"search":null,"stats":null}"#;
        assert_eq!(parse_root(no_flat), Err(RootMiss::Invalid("flat")));
        assert_eq!(parse_root(no_stats), Err(RootMiss::Invalid("stats")));
        assert_eq!(parse_root(no_planes), Err(RootMiss::Invalid("planes")));
    }

    #[test]
    fn parse_root_requires_planes_to_be_an_object() {
        for planes in ["null", r#""x""#, "[]", "1", "true"] {
            assert_eq!(
                parse_root(&root_with("null", "null", planes)),
                Err(RootMiss::Invalid("planes")),
                "planes {planes}"
            );
        }
    }

    #[test]
    fn parse_root_leaves_plane_members_and_search_untyped() {
        for member in [r#""x""#, "[]", "5", "true", "null", "{}", r#"{"sha256":1}"#] {
            let planes = format!(r#"{{"catalog":{member},"enrich":{member},"future":{member}}}"#);
            let body = format!(
                r#"{{"flat":null,"generation":1,"planes":{planes},"schema":1,"search":{member},"stats":null}}"#
            );
            assert!(parse_root(body.as_bytes()).is_ok(), "member {member}");
        }
    }

    #[test]
    fn parse_root_ignores_expires_and_unknown_members() {
        let body = br#"{"expires":"2020-01-01T00:00:00Z","flat":null,"future":{"a":1},"generation":1,"planes":{},"schema":1,"search":null,"stats":null}"#;
        assert!(parse_root(body).is_ok());
    }

    #[test]
    fn parse_root_carries_generation_without_typing_it() {
        let root = parse_root(&root_json(&flat_ref())).unwrap();
        assert_eq!(root.generation, 1);
        for generation in [r#""later""#, "-1", "9007199254740992", "1.5", "null"] {
            let body = format!(
                r#"{{"flat":null,"generation":{generation},"planes":{{}},"schema":1,"search":null,"stats":null}}"#
            );
            assert_eq!(parse_root(body.as_bytes()).unwrap().generation, 0, "{generation}");
        }
        let body = br#"{"flat":null,"planes":{},"schema":1,"search":null,"stats":null}"#;
        assert_eq!(parse_root(body).unwrap().generation, 0);
    }

    #[test]
    fn is_digest_accepts_only_64_lowercase_hex() {
        assert!(is_digest(DIGEST));
        assert!(!is_digest(&DIGEST.to_uppercase()));
        assert!(!is_digest(&DIGEST[..63]));
        assert!(!is_digest(&format!("{DIGEST}0")));
        assert!(!is_digest(&DIGEST.replace('a', "g")));
        assert!(!is_digest(&format!("../{}", &DIGEST[3..])));
    }
}
