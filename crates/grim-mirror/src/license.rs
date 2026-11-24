// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! Licence evidence, the text matcher and the publish decision.

use std::collections::{BTreeSet, HashSet};
use std::path::PathBuf;
use std::sync::OnceLock;

use crate::model::Refusal;
use crate::report::{LicenseSource, Reason, Warning};

/// A canonical SPDX expression.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Expr(pub String);

impl Expr {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Evidence {
    Absent,
    Recognised(Expr),
    /// A licence file the matcher could not name.
    Unrecognised(PathBuf),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Unit,
    Plugin,
    Repository,
}

/// Every evidence source found at one level; callers pass levels in decision order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LevelEvidence {
    pub level: Level,
    pub sources: Vec<Evidence>,
}

/// What the operator declared for a unit, kept apart because only `unit` may
/// stand in for an unrecognised licence file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Declared<'a> {
    /// The `unit-licenses` entry for the unit.
    pub unit: Option<&'a str>,
    /// The entry-level `license`.
    pub entry: Option<&'a str>,
}

/// The `[mirror] licenses` allowlist.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Allowlist {
    /// `None` admits every OSI-approved id.
    pub ids: Option<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LicenseVerdict {
    Accepted {
        expr: Expr,
        source: LicenseSource,
        warnings: Vec<Warning>,
    },
    Refused(Refusal),
}

/// Reads evidence from a licence file's text (SPDX line, then text matcher).
pub fn evidence_from_file(path: PathBuf, text: &str) -> Evidence {
    if let Some(value) = spdx_line_value(text) {
        return match canonical_expression(value) {
            Some(expr) => Evidence::Recognised(expr),
            None => Evidence::Unrecognised(path),
        };
    }
    match match_text(text) {
        TextMatch::Id(id) => Evidence::Recognised(Expr(id.to_owned())),
        TextMatch::Copyleft(_) | TextMatch::Ambiguous | TextMatch::None => Evidence::Unrecognised(path),
    }
}

/// What the text matcher made of a licence text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextMatch {
    /// Exactly one template matched.
    Id(&'static str),
    /// Only a GPL-family template matched (base id, no suffix): the text cannot say `-only` or
    /// `-or-later`, so it only corroborates a field or SPDX line naming the id.
    Copyleft(&'static str),
    /// Two or more distinct ids matched.
    Ambiguous,
    None,
}

/// Longest text the matcher reads; an upstream file past it is unrecognised.
const MAX_MATCH_BYTES: usize = 256 * 1024;
const HEADER_LINES: usize = 10;
/// Required share of a template's own bigrams, in percent.
const COVERAGE_PERCENT: usize = 97;
/// Templates under this many bigrams (MIT, MIT-0, BSD, ISC, 0BSD, Unlicense) tolerate two
/// foreign bigrams: a clause as short as "No AI training." must not hide inside the slack.
const SHORT_TEMPLATE_BIGRAMS: usize = 300;
const SHORT_EXTRA_BIGRAMS: usize = 2;
/// A header line longer than this is prose, never a copyright or title line.
const HEADER_LINE_WORDS: usize = 14;
/// Words that make a line a clause. A header line carrying one is kept, and a foreign bigram
/// carrying one refuses the match, so a restriction cannot ride along as a dropped header or
/// inside the slack a long template allows.
const CLAUSE_WORDS: &[&str] = &[
    "not",
    "no",
    "non",
    "noncommercial",
    "commercial",
    "commercially",
    "only",
    "except",
    "exception",
    "prohibit",
    "prohibited",
    "prohibits",
    "prohibition",
    "forbid",
    "forbidden",
    "never",
    "without",
    "cannot",
    "nor",
    "train",
    "training",
    "ai",
    "unless",
];

fn is_clause_word(w: &str) -> bool {
    CLAUSE_WORDS.contains(&w)
}

/// Template texts: `(id, text)`; two Apache-2.0 entries (with and without the appendix).
const TEMPLATES: &[(&str, &str)] = &[
    ("MIT", include_str!("../licenses/MIT.txt")),
    ("MIT-0", include_str!("../licenses/MIT-0.txt")),
    ("Apache-2.0", include_str!("../licenses/Apache-2.0.txt")),
    ("Apache-2.0", include_str!("../licenses/Apache-2.0-noappendix.txt")),
    ("BSD-2-Clause", include_str!("../licenses/BSD-2-Clause.txt")),
    ("BSD-3-Clause", include_str!("../licenses/BSD-3-Clause.txt")),
    ("ISC", include_str!("../licenses/ISC.txt")),
    ("0BSD", include_str!("../licenses/0BSD.txt")),
    ("MPL-2.0", include_str!("../licenses/MPL-2.0.txt")),
    ("Unlicense", include_str!("../licenses/Unlicense.txt")),
    ("CC0-1.0", include_str!("../licenses/CC0-1.0.txt")),
    ("GPL-2.0", include_str!("../licenses/GPL-2.0.txt")),
    ("GPL-3.0", include_str!("../licenses/GPL-3.0.txt")),
    ("LGPL-2.1", include_str!("../licenses/LGPL-2.1.txt")),
    ("LGPL-3.0", include_str!("../licenses/LGPL-3.0.txt")),
    ("AGPL-3.0", include_str!("../licenses/AGPL-3.0.txt")),
];

/// One normalised line: its words, and whether it is a header line to drop
/// (a copyright line or a title naming the licence).
struct Line {
    words: Vec<String>,
    droppable: bool,
}

struct Template {
    id: &'static str,
    bigrams: HashSet<String>,
}

/// Words a droppable title may hold: licence names, versions and SPDX labels. Any other word
/// ("personal", "for", "use") is prose that could carry a restriction, so the line is kept.
const TITLE_WORDS: &[&str] = &[
    "the",
    "mit",
    "license",
    "licence",
    "apache",
    "version",
    "bsd",
    "clause",
    "simplified",
    "new",
    "revised",
    "modified",
    "isc",
    "zero",
    "mozilla",
    "public",
    "mpl",
    "unlicense",
    "cc0",
    "creative",
    "commons",
    "universal",
    "gnu",
    "general",
    "lesser",
    "library",
    "affero",
    "gpl",
    "lgpl",
    "agpl",
    "spdx",
    "identifier",
];

fn is_title_word(w: &str) -> bool {
    TITLE_WORDS.contains(&w) || w == "v" || w.chars().all(|c| c.is_ascii_digit())
}

fn lines_of(text: &str) -> Vec<Line> {
    lines_with(text, true)
}

/// `strict_title` off keeps the loose "short line naming a licence" rule, which the template
/// side needs so its bigram sets stay as they were.
fn lines_with(text: &str, strict_title: bool) -> Vec<Line> {
    text.lines()
        .filter_map(|raw| {
            let lower = raw.to_lowercase();
            let words: Vec<String> = lower
                .split(|c: char| !c.is_alphanumeric())
                .filter(|w| !w.is_empty())
                .map(str::to_owned)
                .collect();
            if words.is_empty() {
                return None;
            }
            let joined = words.join(" ");
            let copyright = lower.contains("copyright")
                || lower.contains('\u{a9}')
                || lower.contains("(c)")
                || joined.contains("all rights reserved");
            let no_attribution = joined.starts_with("mit no attribution");
            let title = words.len() <= 10
                && (lower.contains("licen") || no_attribution)
                && (!strict_title || no_attribution || words.iter().all(|w| is_title_word(w)));
            // "MIT No Attribution" is a licence name, not a restriction.
            let names_clause = words.iter().enumerate().any(|(i, w)| {
                is_clause_word(w) && !(w == "no" && words.get(i + 1).is_some_and(|n| n == "attribution"))
            });
            let plain = words.len() <= HEADER_LINE_WORDS && !names_clause;
            Some(Line {
                words,
                droppable: (copyright || title) && plain,
            })
        })
        .collect()
}

fn bigrams_of<'a>(words: impl Iterator<Item = &'a String>) -> HashSet<String> {
    let words: Vec<&String> = words.collect();
    words.windows(2).map(|w| format!("{} {}", w[0], w[1])).collect()
}

fn templates() -> &'static [Template] {
    static CACHE: OnceLock<Vec<Template>> = OnceLock::new();
    CACHE.get_or_init(|| {
        TEMPLATES
            .iter()
            .map(|(id, text)| {
                // A template's leading title and placeholder copyright lines
                // go like a file's; only the leading run, so a body line that
                // mentions copyright stays.
                let mut leading = true;
                let kept = lines_with(text, false).into_iter().enumerate().filter_map(|(i, l)| {
                    let header = l.words.len() <= 12 && l.droppable || i == 0 && l.words.len() <= 6;
                    leading &= header;
                    (!leading).then_some(l.words)
                });
                let words: Vec<String> = kept.flatten().collect();
                Template {
                    id,
                    bigrams: bigrams_of(words.iter()),
                }
            })
            .collect()
    })
}

/// The file's words after dropping header lines: the header is the leading
/// block, ending at the first line whose bigrams all occur in the template
/// and capped at ten lines.
fn file_bigrams(lines: &[Line], template: &HashSet<String>) -> HashSet<String> {
    let in_template = |l: &Line| {
        l.words.len() >= 3
            && l.words
                .windows(2)
                .all(|w| template.contains(&format!("{} {}", w[0], w[1])))
    };
    let mut in_header = true;
    let mut kept: Vec<&String> = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        if in_header && (i >= HEADER_LINES || in_template(line)) {
            in_header = false;
        }
        if in_header && line.droppable {
            continue;
        }
        kept.extend(line.words.iter());
    }
    bigrams_of(kept.into_iter())
}

fn covers(file: &HashSet<String>, template: &HashSet<String>) -> bool {
    let n = template.len();
    let covered = template.iter().filter(|b| file.contains(*b)).count();
    let extra: Vec<&String> = file.iter().filter(|b| !template.contains(*b)).collect();
    let bound = if n < SHORT_TEMPLATE_BIGRAMS {
        SHORT_EXTRA_BIGRAMS
    } else {
        4.max(n.div_ceil(50))
    };
    let restrictive = extra.iter().any(|b| b.split(' ').any(is_clause_word));
    n > 0 && covered * 100 >= COVERAGE_PERCENT * n && extra.len() <= bound && !restrictive
}

/// Matches a licence text against the embedded templates.
pub fn match_text(text: &str) -> TextMatch {
    if text.len() > MAX_MATCH_BYTES {
        return TextMatch::None;
    }
    let lines = lines_of(text);
    let mut ids: Vec<&'static str> = Vec::new();
    for t in templates() {
        if !ids.contains(&t.id) && covers(&file_bigrams(&lines, &t.bigrams), &t.bigrams) {
            ids.push(t.id);
        }
    }
    match ids.as_slice() {
        [] => TextMatch::None,
        [id] if is_copyleft(id) => TextMatch::Copyleft(id),
        [id] => TextMatch::Id(id),
        _ => TextMatch::Ambiguous,
    }
}

fn is_copyleft(id: &str) -> bool {
    id.contains("GPL")
}

/// The value of the first `SPDX-License-Identifier:` line, comment closers trimmed.
fn spdx_line_value(text: &str) -> Option<&str> {
    const TAG: &str = "SPDX-License-Identifier:";
    let line = text.lines().find(|l| l.contains(TAG))?;
    let value = line.split_once(TAG)?.1.trim();
    let value = value.trim_end_matches("*/").trim_end_matches("-->").trim();
    Some(value)
}

/// Parses `value` as a lax SPDX expression and renders it canonically:
/// GNU ids carry `-only` or `-or-later`, operators are upper case.
pub(crate) fn canonical_expression(value: &str) -> Option<Expr> {
    use spdx::expression::{ExprNode, Operator};

    let parsed = spdx::Expression::parse_mode(value, spdx::ParseMode::LAX).ok()?;
    // Postfix to infix; the bool marks an OR node, which an AND parent parenthesises.
    let mut stack: Vec<(String, bool)> = Vec::new();
    for node in parsed.iter() {
        match node {
            ExprNode::Req(r) => {
                let mut s = r.req.license.to_string();
                if let spdx::LicenseItem::Spdx { id, or_later } = &r.req.license
                    && id.is_gnu()
                    && id.is_deprecated()
                    && !or_later
                {
                    s.push_str("-only");
                }
                if let Some(add) = &r.req.addition {
                    s.push_str(&format!(" WITH {add}"));
                }
                stack.push((s, false));
            }
            ExprNode::Op(op) => {
                let (b, b_or) = stack.pop()?;
                let (a, a_or) = stack.pop()?;
                let rendered = match op {
                    Operator::And => {
                        let wrap = |s: String, or: bool| if or { format!("({s})") } else { s };
                        (format!("{} AND {}", wrap(a, a_or), wrap(b, b_or)), false)
                    }
                    Operator::Or => (format!("{a} OR {b}"), true),
                };
                stack.push(rendered);
            }
        }
    }
    let (expr, _) = stack.pop()?;
    stack.is_empty().then_some(Expr(expr))
}

/// Reads evidence from a manifest `license` field (lax SPDX parse); a value
/// that does not parse is no evidence.
pub fn evidence_from_field(field: &str) -> Evidence {
    canonical_expression(field.trim()).map_or(Evidence::Absent, Evidence::Recognised)
}

/// `GPL-3.0-only`, `GPL-3.0-or-later` and `GPL-3.0` all reduce to `GPL-3.0`;
/// a compound expression names no single id.
fn base_id(expr: &Expr) -> Option<&str> {
    let s = expr.as_str();
    if s.contains([' ', '(']) {
        return None;
    }
    Some(
        s.trim_end_matches("-or-later")
            .trim_end_matches("-only")
            .trim_end_matches('+'),
    )
}

/// Builds the evidence of one level from its manifest fields and licence files
/// (path, text). A GPL-family text alone stays unrecognised; it becomes the
/// recognised expression of a field or SPDX line at the same level that names its id.
pub fn level_evidence(level: Level, fields: &[&str], files: &[(PathBuf, &str)]) -> LevelEvidence {
    let mut sources: Vec<Evidence> = fields.iter().map(|f| evidence_from_field(f)).collect();
    let file_evidence: Vec<Evidence> = files
        .iter()
        .map(|(path, text)| evidence_from_file(path.clone(), text))
        .collect();
    let named: Vec<Expr> = sources
        .iter()
        .chain(&file_evidence)
        .filter_map(|e| match e {
            Evidence::Recognised(x) => Some(x.clone()),
            _ => None,
        })
        .collect();
    for ((_, text), ev) in files.iter().zip(file_evidence) {
        let corroborated = match (&ev, match_text(text)) {
            // An SPDX line that did not parse is never rescued by the text.
            (Evidence::Unrecognised(_), TextMatch::Copyleft(base)) if spdx_line_value(text).is_none() => {
                named.iter().find(|x| base_id(x) == Some(base))
            }
            _ => None,
        };
        sources.push(corroborated.map_or(ev, |x| Evidence::Recognised(x.clone())));
    }
    LevelEvidence { level, sources }
}

enum Combined {
    Absent,
    Recognised(Vec<Expr>),
    Unrecognised,
}

fn combine(sources: &[Evidence]) -> Combined {
    if sources.iter().any(|e| matches!(e, Evidence::Unrecognised(_))) {
        return Combined::Unrecognised;
    }
    let exprs: Vec<Expr> = sources
        .iter()
        .filter_map(|e| match e {
            Evidence::Recognised(x) => Some(x.clone()),
            _ => None,
        })
        .collect();
    if exprs.is_empty() {
        Combined::Absent
    } else {
        Combined::Recognised(exprs)
    }
}

/// Whether the allowlist admits `expr`: `OR` needs one admitted branch, `AND`
/// all of them, `WITH` and `+` are judged on the base id, and a `LicenseRef-*`
/// is never admitted.
fn is_allowed(expr: &Expr, allow: &Allowlist) -> bool {
    let Ok(parsed) = spdx::Expression::parse(expr.as_str()) else {
        return false;
    };
    let listed: Option<HashSet<String>> = allow.ids.as_ref().map(|ids| {
        ids.iter()
            .filter_map(|i| canonical_expression(i))
            .map(|e| e.0)
            .collect()
    });
    parsed.evaluate(|req| match &req.license {
        spdx::LicenseItem::Spdx { id, .. } => match &listed {
            None => id.is_osi_approved(),
            Some(set) => set.contains(&req.license.to_string()),
        },
        spdx::LicenseItem::Other { .. } => false,
    })
}

fn refuse(reason: Reason, message: impl Into<String>) -> LicenseVerdict {
    LicenseVerdict::Refused(Refusal {
        reason,
        message: message.into(),
    })
}

fn admit(expr: Expr, source: LicenseSource, allow: &Allowlist, warnings: Vec<Warning>) -> LicenseVerdict {
    if is_allowed(&expr, allow) {
        LicenseVerdict::Accepted { expr, source, warnings }
    } else {
        refuse(
            Reason::LicenseNotAllowed,
            format!("{} is not admitted by the allowlist", expr.as_str()),
        )
    }
}

/// Decides the licence of a unit: the first level with evidence decides, and
/// only when every level is absent does a declaration supply the licence.
pub fn decide(levels: &[LevelEvidence], declared: Declared<'_>, allow: &Allowlist) -> LicenseVerdict {
    let declared_unit = declared.unit.and_then(canonical_expression);
    for level in levels {
        match combine(&level.sources) {
            Combined::Absent => {}
            Combined::Recognised(exprs) => {
                if let Some(other) = exprs.iter().find(|e| **e != exprs[0]) {
                    return refuse(
                        Reason::LicenseConflict,
                        format!("{} conflicts with {}", exprs[0].as_str(), other.as_str()),
                    );
                }
                let ignored = declared.unit.or(declared.entry).is_some();
                let warnings = if ignored {
                    vec![Warning {
                        code: "declaration-ignored".into(),
                        message: format!("a declared licence is ignored; detected {}", exprs[0].as_str()),
                    }]
                } else {
                    Vec::new()
                };
                return admit(exprs[0].clone(), LicenseSource::Detected, allow, warnings);
            }
            Combined::Unrecognised => {
                return match declared_unit {
                    Some(expr) => admit(expr, LicenseSource::Declared, allow, Vec::new()),
                    None => refuse(
                        Reason::LicenseUnrecognised,
                        "a licence file matches no known licence; declare it in unit-licenses",
                    ),
                };
            }
        }
    }
    match declared_unit.or_else(|| declared.entry.and_then(canonical_expression)) {
        Some(expr) => admit(expr, LicenseSource::Declared, allow, Vec::new()),
        None => refuse(Reason::LicenseMissing, "no licence found; add unit-licenses"),
    }
}

/// The bundle expression: members parenthesised, deduplicated, sorted, joined by ` AND `.
pub fn join_bundle(exprs: &[Expr]) -> Expr {
    let distinct: BTreeSet<&Expr> = exprs.iter().collect();
    Expr(
        distinct
            .into_iter()
            .map(|e| format!("({})", e.as_str()))
            .collect::<Vec<_>>()
            .join(" AND "),
    )
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use std::fs;

    fn corpus_dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/licenses")
    }

    fn evidence(text: &str) -> Evidence {
        evidence_from_file(PathBuf::from("LICENSE"), text)
    }

    fn template(id: &str) -> &'static str {
        TEMPLATES.iter().find(|(i, _)| *i == id).unwrap().1
    }

    #[test]
    fn corpus_manifest_pins_every_outcome() {
        let manifest = fs::read_to_string(corpus_dir().join("manifest.txt")).unwrap();
        let (mut must_match, mut must_not) = (0, 0);
        for line in manifest.lines().filter(|l| !l.starts_with('#') && !l.is_empty()) {
            let (file, outcome) = line.split_once('\t').unwrap();
            let text = fs::read_to_string(corpus_dir().join(file)).unwrap();
            let got = evidence_from_file(PathBuf::from(file), &text);
            match outcome {
                "unrecognised" => {
                    must_not += 1;
                    assert_eq!(got, Evidence::Unrecognised(PathBuf::from(file)), "{file}");
                }
                gpl if gpl.starts_with("gpl:") => {
                    must_match += 1;
                    assert!(
                        matches!(match_text(&text), TextMatch::Copyleft(id) if id == &gpl[4..]),
                        "{file}"
                    );
                    assert_eq!(got, Evidence::Unrecognised(PathBuf::from(file)), "{file}");
                }
                id => {
                    must_match += 1;
                    assert_eq!(got, Evidence::Recognised(Expr(id.to_owned())), "{file}");
                }
            }
        }
        // Guards against a manifest that silently lost lines.
        assert_eq!(must_match, 69, "must-match {must_match}");
        assert_eq!(must_not, 21, "must-not-match {must_not}");
    }

    #[test]
    fn every_template_is_covered_by_the_corpus() {
        let manifest = fs::read_to_string(corpus_dir().join("manifest.txt")).unwrap();
        for (id, _) in TEMPLATES {
            let wanted = id.trim_end_matches("-noappendix");
            assert!(
                manifest
                    .lines()
                    .any(|l| l.ends_with(&format!("\t{wanted}")) || l.ends_with(&format!("\tgpl:{wanted}"))),
                "{wanted}"
            );
        }
    }

    #[test]
    fn near_twin_templates_do_not_match_each_other() {
        let tpl = |id| templates().iter().find(|t| t.id == id).unwrap().bigrams.clone();
        let file_of = |text: &str, against: &HashSet<String>| file_bigrams(&lines_of(text), against);
        for (a, b) in [
            ("BSD-3-Clause", "BSD-2-Clause"),
            ("BSD-2-Clause", "BSD-3-Clause"),
            ("MIT", "MIT-0"),
            ("MIT-0", "MIT"),
            ("ISC", "0BSD"),
            ("0BSD", "ISC"),
            ("LGPL-3.0", "GPL-3.0"),
            ("GPL-3.0", "AGPL-3.0"),
        ] {
            let against = tpl(b);
            assert!(
                !covers(&file_of(template(a), &against), &against),
                "{a} text matched the {b} template"
            );
        }
    }

    #[test]
    fn coverage_boundary_is_four_missing_bigrams_for_mit() {
        // Each deleted interior word removes two bigrams the template has.
        let two = template("MIT").replace("sublicense, ", "").replace("merge, ", "");
        assert_eq!(evidence(&two), Evidence::Recognised(Expr("MIT".into())));
        // The first word costs one more bigram: five missing, over the bound.
        let five = two.replacen("Permission ", "", 1);
        assert_eq!(evidence(&five), Evidence::Unrecognised(PathBuf::from("LICENSE")));
        let six = two.replace("NONINFRINGEMENT. ", "");
        assert_eq!(evidence(&six), Evidence::Unrecognised(PathBuf::from("LICENSE")));
    }

    #[test]
    fn extra_bigram_boundary_is_two_for_mit() {
        // Each appended word adds one bigram the template lacks.
        let two = format!("{}alpha beta\n", template("MIT"));
        assert_eq!(evidence(&two), Evidence::Recognised(Expr("MIT".into())));
        let three = format!("{}alpha beta gamma\n", template("MIT"));
        assert_eq!(evidence(&three), Evidence::Unrecognised(PathBuf::from("LICENSE")));
    }

    #[test]
    fn long_template_tolerates_harmless_extras_but_not_a_clause_word() {
        let harmless = format!(
            "{}\nSee the project website for details of this release.\n",
            template("Apache-2.0")
        );
        assert_eq!(evidence(&harmless), Evidence::Recognised(Expr("Apache-2.0".into())));
        let clause = format!("{}\nUse is limited to research only.\n", template("Apache-2.0"));
        assert_eq!(evidence(&clause), Evidence::Unrecognised(PathBuf::from("LICENSE")));
    }

    #[test]
    fn gpl_text_alone_is_unrecognised_but_named() {
        assert_eq!(match_text(template("GPL-3.0")), TextMatch::Copyleft("GPL-3.0"));
        assert_eq!(
            evidence(template("GPL-3.0")),
            Evidence::Unrecognised(PathBuf::from("LICENSE"))
        );
    }

    #[test]
    fn oversized_text_is_unrecognised() {
        let big = format!("{}{}", template("MIT"), " ".repeat(MAX_MATCH_BYTES));
        assert_eq!(match_text(&big), TextMatch::None);
    }

    #[test]
    fn spdx_line_names_the_licence() {
        let text = "/* SPDX-License-Identifier: Apache-2.0 */\nanything else\n";
        assert_eq!(evidence(text), Evidence::Recognised(Expr("Apache-2.0".into())));
        let both = format!("SPDX-License-Identifier: BSD-3-Clause\n{}", template("MIT"));
        assert_eq!(evidence(&both), Evidence::Recognised(Expr("BSD-3-Clause".into())));
    }

    #[test]
    fn spdx_line_is_read_lax_and_canonical() {
        let e = |v: &str| evidence(&format!("SPDX-License-Identifier: {v}\n"));
        assert_eq!(e("GPL-3.0"), Evidence::Recognised(Expr("GPL-3.0-only".into())));
        assert_eq!(e("GPL-2.0+"), Evidence::Recognised(Expr("GPL-2.0-or-later".into())));
        assert_eq!(
            e("MIT OR Apache-2.0"),
            Evidence::Recognised(Expr("MIT OR Apache-2.0".into()))
        );
        assert_eq!(
            e("(MIT OR Apache-2.0) AND BSD-3-Clause"),
            Evidence::Recognised(Expr("(MIT OR Apache-2.0) AND BSD-3-Clause".into()))
        );
        assert_eq!(
            e("GPL-2.0-only WITH Classpath-exception-2.0"),
            Evidence::Recognised(Expr("GPL-2.0-only WITH Classpath-exception-2.0".into()))
        );
    }

    #[test]
    fn spdx_line_that_does_not_parse_is_unrecognised() {
        // The matcher must not rescue it: the file said something else than a licence.
        let text = format!(
            "SPDX-License-Identifier: Complete terms in LICENSE.txt\n{}",
            template("MIT")
        );
        assert_eq!(evidence(&text), Evidence::Unrecognised(PathBuf::from("LICENSE")));
    }

    fn expr(s: &str) -> Expr {
        Expr(s.into())
    }

    fn rec(s: &str) -> Evidence {
        Evidence::Recognised(expr(s))
    }

    fn unrec() -> Evidence {
        Evidence::Unrecognised(PathBuf::from("LICENSE"))
    }

    fn lvl(level: Level, sources: Vec<Evidence>) -> LevelEvidence {
        LevelEvidence { level, sources }
    }

    fn osi() -> Allowlist {
        Allowlist { ids: None }
    }

    fn list(ids: &[&str]) -> Allowlist {
        Allowlist {
            ids: Some(ids.iter().map(|s| (*s).to_owned()).collect()),
        }
    }

    fn accepted(v: &LicenseVerdict) -> (&str, LicenseSource, usize) {
        match v {
            LicenseVerdict::Accepted { expr, source, warnings } => (expr.as_str(), *source, warnings.len()),
            LicenseVerdict::Refused(r) => panic!("refused: {r:?}"),
        }
    }

    fn reason(v: &LicenseVerdict) -> Reason {
        match v {
            LicenseVerdict::Refused(r) => r.reason,
            LicenseVerdict::Accepted { .. } => panic!("accepted"),
        }
    }

    const NONE: Declared<'static> = Declared {
        unit: None,
        entry: None,
    };

    #[test]
    fn field_is_lax_canonical_and_unparseable_is_absent() {
        assert_eq!(evidence_from_field("GPL-2.0+"), rec("GPL-2.0-or-later"));
        assert_eq!(evidence_from_field("MIT/Apache-2.0"), rec("MIT OR Apache-2.0"));
        assert_eq!(evidence_from_field("Complete terms in LICENSE.txt"), Evidence::Absent);
    }

    #[test]
    fn detected_file_publishes_as_detected() {
        let v = decide(&[lvl(Level::Unit, vec![rec("MIT")])], NONE, &osi());
        assert_eq!(accepted(&v), ("MIT", LicenseSource::Detected, 0));
    }

    #[test]
    fn declared_entry_license_with_nothing_detected_is_declared() {
        let levels = [lvl(Level::Unit, vec![]), lvl(Level::Repository, vec![Evidence::Absent])];
        let d = Declared {
            unit: None,
            entry: Some("Apache-2.0"),
        };
        assert_eq!(
            accepted(&decide(&levels, d, &osi())),
            ("Apache-2.0", LicenseSource::Declared, 0)
        );
    }

    #[test]
    fn unit_licenses_beats_entry_license_when_all_absent() {
        let d = Declared {
            unit: Some("ISC"),
            entry: Some("MIT"),
        };
        assert_eq!(accepted(&decide(&[], d, &osi())).0, "ISC");
    }

    #[test]
    fn nothing_anywhere_is_missing() {
        let levels = [lvl(Level::Unit, vec![Evidence::Absent])];
        assert_eq!(reason(&decide(&levels, NONE, &osi())), Reason::LicenseMissing);
    }

    #[test]
    fn first_non_absent_level_decides() {
        let levels = [
            lvl(Level::Unit, vec![]),
            lvl(Level::Plugin, vec![rec("MIT")]),
            lvl(Level::Repository, vec![rec("Apache-2.0")]),
        ];
        assert_eq!(accepted(&decide(&levels, NONE, &osi())).0, "MIT");
        // An unrecognised level above a recognised one still decides.
        let levels = [
            lvl(Level::Unit, vec![unrec()]),
            lvl(Level::Repository, vec![rec("MIT")]),
        ];
        assert_eq!(reason(&decide(&levels, NONE, &osi())), Reason::LicenseUnrecognised);
    }

    #[test]
    fn unrecognised_file_overrides_recognised_source_in_its_level() {
        let levels = [lvl(Level::Unit, vec![rec("MIT"), unrec()])];
        assert_eq!(reason(&decide(&levels, NONE, &osi())), Reason::LicenseUnrecognised);
    }

    #[test]
    fn differing_recognised_sources_conflict_and_equal_ones_do_not() {
        let levels = [lvl(Level::Unit, vec![rec("MIT"), rec("Apache-2.0")])];
        assert_eq!(reason(&decide(&levels, NONE, &osi())), Reason::LicenseConflict);
        let levels = [lvl(Level::Unit, vec![rec("MIT"), rec("MIT")])];
        assert_eq!(accepted(&decide(&levels, NONE, &osi())).0, "MIT");
    }

    #[test]
    fn unrecognised_is_refused_unless_the_unit_is_declared() {
        let levels = [lvl(Level::Unit, vec![unrec()])];
        assert_eq!(reason(&decide(&levels, NONE, &osi())), Reason::LicenseUnrecognised);
        let d = Declared {
            unit: Some("MIT"),
            entry: None,
        };
        assert_eq!(
            accepted(&decide(&levels, d, &osi())),
            ("MIT", LicenseSource::Declared, 0)
        );
    }

    #[test]
    fn entry_license_does_not_override_an_unrecognised_file() {
        let levels = [lvl(Level::Unit, vec![unrec()])];
        let d = Declared {
            unit: None,
            entry: Some("MIT"),
        };
        assert_eq!(reason(&decide(&levels, d, &osi())), Reason::LicenseUnrecognised);
    }

    #[test]
    fn declaration_beside_detection_warns_and_detected_wins() {
        let levels = [lvl(Level::Plugin, vec![rec("MIT")])];
        for d in [
            Declared {
                unit: Some("Apache-2.0"),
                entry: None,
            },
            Declared {
                unit: None,
                entry: Some("Apache-2.0"),
            },
        ] {
            let v = decide(&levels, d, &osi());
            assert_eq!(accepted(&v), ("MIT", LicenseSource::Detected, 1));
            let LicenseVerdict::Accepted { warnings, .. } = v else {
                unreachable!()
            };
            assert_eq!(warnings[0].code, "declaration-ignored");
        }
    }

    #[test]
    fn mit_plus_non_commercial_clause_is_refused_unrecognised() {
        let text = format!(
            "{}\nThis software may not be used for commercial purposes of any kind.\n",
            template("MIT")
        );
        let file = vec![(PathBuf::from("LICENSE"), text.as_str())];
        let l = level_evidence(Level::Unit, &[], &file);
        assert_eq!(reason(&decide(&[l], NONE, &osi())), Reason::LicenseUnrecognised);
    }

    #[test]
    fn matcher_hit_publishes_detected_end_to_end() {
        let file = vec![(PathBuf::from("LICENSE"), template("MIT"))];
        let levels = [level_evidence(Level::Unit, &[], &file)];
        assert_eq!(accepted(&decide(&levels, NONE, &osi())).0, "MIT");
    }

    #[test]
    fn gpl_text_alone_is_unrecognised() {
        let file = vec![(PathBuf::from("LICENSE"), template("GPL-3.0"))];
        let l = level_evidence(Level::Unit, &[], &file);
        assert_eq!(l.sources, vec![unrec()]);
        assert_eq!(reason(&decide(&[l], NONE, &osi())), Reason::LicenseUnrecognised);
    }

    #[test]
    fn gpl_text_is_corroborated_by_a_field_or_spdx_line_naming_the_id() {
        let file = vec![(PathBuf::from("LICENSE"), template("GPL-3.0"))];
        let l = level_evidence(Level::Unit, &["GPL-3.0"], &file);
        assert_eq!(l.sources, vec![rec("GPL-3.0-only"), rec("GPL-3.0-only")]);
        assert_eq!(accepted(&decide(&[l], NONE, &osi())).0, "GPL-3.0-only");

        let l = level_evidence(Level::Unit, &["GPL-3.0-or-later"], &file);
        assert_eq!(accepted(&decide(&[l], NONE, &osi())).0, "GPL-3.0-or-later");

        // Another file at the level carries the SPDX line.
        let both = vec![
            (PathBuf::from("LICENSE"), template("GPL-3.0")),
            (PathBuf::from("COPYING"), "SPDX-License-Identifier: GPL-3.0-or-later\n"),
        ];
        let l = level_evidence(Level::Unit, &[], &both);
        assert_eq!(accepted(&decide(&[l], NONE, &osi())).0, "GPL-3.0-or-later");
    }

    #[test]
    fn gpl_text_is_not_corroborated_by_another_id() {
        let file = vec![(PathBuf::from("LICENSE"), template("GPL-3.0"))];
        for field in ["GPL-2.0", "MIT", "LGPL-3.0", "AGPL-3.0", "GPL-3.0 OR MIT"] {
            let l = level_evidence(Level::Unit, &[field], &file);
            assert_eq!(
                reason(&decide(&[l], NONE, &osi())),
                Reason::LicenseUnrecognised,
                "{field}"
            );
        }
    }

    #[test]
    fn default_allowlist_is_osi_approved_ids() {
        let levels = [lvl(Level::Unit, vec![rec("CC-BY-NC-4.0")])];
        assert_eq!(reason(&decide(&levels, NONE, &osi())), Reason::LicenseNotAllowed);
        let levels = [lvl(Level::Unit, vec![rec("Apache-2.0")])];
        assert_eq!(accepted(&decide(&levels, NONE, &osi())).0, "Apache-2.0");
        // Not allowed whatever is declared.
        let d = Declared {
            unit: Some("MIT"),
            entry: Some("MIT"),
        };
        let levels = [lvl(Level::Unit, vec![rec("CC-BY-NC-4.0")])];
        assert_eq!(reason(&decide(&levels, d, &osi())), Reason::LicenseNotAllowed);
    }

    #[test]
    fn a_declared_licence_must_be_allowed_too() {
        let d = Declared {
            unit: None,
            entry: Some("CC-BY-NC-4.0"),
        };
        assert_eq!(reason(&decide(&[], d, &osi())), Reason::LicenseNotAllowed);
    }

    #[test]
    fn configured_list_replaces_the_default() {
        assert!(is_allowed(&expr("CC0-1.0"), &list(&["CC0-1.0"])));
        assert!(!is_allowed(&expr("CC0-1.0"), &osi()));
        assert!(!is_allowed(&expr("Apache-2.0"), &list(&["MIT"])));
        // Entries are canonicalised: the deprecated spelling admits the canonical id.
        assert!(is_allowed(&expr("GPL-3.0-only"), &list(&["GPL-3.0"])));
    }

    #[test]
    fn or_needs_one_branch_and_needs_all() {
        let a = list(&["MIT"]);
        assert!(is_allowed(&expr("MIT OR Apache-2.0"), &a));
        assert!(!is_allowed(&expr("MIT AND Apache-2.0"), &a));
        assert!(is_allowed(&expr("(MIT OR Apache-2.0) AND MIT"), &a));
        assert!(!is_allowed(&expr("(Apache-2.0 OR ISC) AND MIT"), &a));
        assert!(is_allowed(&expr("MIT OR CC-BY-NC-4.0"), &osi()));
        assert!(!is_allowed(&expr("MIT AND CC-BY-NC-4.0"), &osi()));
    }

    #[test]
    fn with_and_plus_are_judged_on_the_base_id() {
        let e = expr("GPL-2.0-only WITH Classpath-exception-2.0");
        assert!(is_allowed(&e, &osi()));
        assert!(is_allowed(&e, &list(&["GPL-2.0-only"])));
        assert!(!is_allowed(&e, &list(&["MIT"])));
        assert!(is_allowed(&expr("GPL-2.0-or-later"), &list(&["GPL-2.0+"])));
        assert!(!is_allowed(&expr("GPL-2.0-or-later"), &list(&["GPL-2.0-only"])));
    }

    #[test]
    fn license_ref_is_never_allowed() {
        assert!(!is_allowed(&expr("LicenseRef-custom"), &osi()));
        assert!(!is_allowed(&expr("LicenseRef-custom"), &list(&["LicenseRef-custom"])));
        assert!(!is_allowed(&expr("MIT AND LicenseRef-custom"), &osi()));
    }

    #[test]
    fn bundle_join_parenthesises_dedupes_and_sorts() {
        let out = join_bundle(&[expr("MIT"), expr("Apache-2.0 OR MIT"), expr("MIT")]);
        assert_eq!(out.as_str(), "(Apache-2.0 OR MIT) AND (MIT)");
        assert_eq!(join_bundle(&[expr("MIT")]).as_str(), "(MIT)");
    }
}
