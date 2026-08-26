// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! Detail-pane content and scroll geometry — pure, ratatui-free.
//!
//! [`detail_lines`] builds the pane's semantic lines for a selected row;
//! [`scroll_max`] bounds the vertical scroll offset by counting the
//! post-wrap rows those lines occupy in the live viewport. Both
//! [`super::state`] (clamping the offset at mutation time) and
//! [`super::render`] (projecting + drawing) consume this module, so the
//! content layout and its scroll bound stay one source of truth.
//!
//! The catalog column widths live here too: the detail viewport is
//! whatever is left of the terminal after the catalog takes its fixed
//! width, so the geometry is one concern.

use super::bundle_members::MemberNode;
use super::companion::{Companion, CompanionCache};
use super::state::{ArtifactState, TuiRow};
use crate::config::registry_resolve::RowSource;

/// Catalog column widths (chars) — the projection pads/truncates to
/// these so the table aligns regardless of how long an identifier is.
pub const W_KIND: usize = 8;
pub const W_REPO: usize = 46;
pub const W_TAG: usize = 12;
/// Status column width — wide enough for the longest label
/// (`✘ integrity-missing`, 19 chars) so the header underline spans the
/// full column instead of stopping at `Status`.
pub const W_STATUS: usize = 19;
/// Extra Catalog width reserved for the deprecation marker (` † deprecated`)
/// appended inside the Status column on deprecated rows. Sized to the full
/// marker — leading space + `†` (U+2020, a single monochrome cell) + space +
/// the word `deprecated` (10) = 13 — so it never clips, even when the status
/// label is at its widest (`✘ integrity-missing`).
pub const W_DEPRECATED: usize = 13;
/// Width of the Registry column shown in flat-view multi-registry mode
/// (label + 2-column gap is added on top by [`catalog_width`]). The flat list
/// prepends it when more than one registry is in scope.
pub const W_REGISTRY: usize = 20;
/// Total terminal columns the Catalog needs to show every fixed-width
/// column un-truncated: 2 (mark) + repo + 2 + kind + 2 + tag + 2 + status,
/// plus room for the trailing deprecation marker and 2 block borders.
/// Selection is shown by row highlight (no leading symbol). Sized to exactly
/// this side-by-side so Detail gets all slack. Excludes the optional Registry
/// column — [`catalog_width`] adds it when that column is shown.
pub const CATALOG_WIDTH: u16 =
    (2 + W_REPO + 2 + W_KIND + 2 + W_TAG + 2 + W_STATUS + 2 + W_DEPRECATED) as u16 + 2 /* borders */;
/// Narrowest usable Detail column (the side-by-side layout threshold).
pub const DETAIL_MIN_WIDTH: u16 = 30;

/// The Catalog's needed width for the current view. The flat multi-registry
/// list prepends a Registry column (`W_REGISTRY` + a 2-column gap), so the
/// Catalog block must be that much wider or the rightmost Status column (and
/// its deprecation marker) overflows the fixed block and clips. Single source
/// of truth for the side-by-side split threshold and the Catalog's fixed width.
pub fn catalog_width(show_registry_column: bool) -> u16 {
    CATALOG_WIDTH
        + if show_registry_column {
            (W_REGISTRY + 2) as u16
        } else {
            0
        }
}

/// One semantic line of the Detail pane. Pure data — `draw` maps each
/// kind to concrete styling with zero logic of its own.
///
/// Closed internal enum — matches stay total, no `#[non_exhaustive]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DetailLine {
    /// Blank spacer.
    Blank,
    /// The artifact reference — centered, bold, accent color.
    Identifier(String),
    /// An underlined section label; the colon is part of the label
    /// (e.g. `Summary:`).
    SectionLabel(&'static str),
    /// `label value` on one line; the label includes the colon
    /// (e.g. `Keywords:`).
    MetaEntry {
        /// The highlighted key, colon included.
        label: &'static str,
        /// The plain value rendered on the same line.
        value: String,
    },
    /// Plain wrapped body text.
    Text(String),
    /// A markdown heading from a companion document. `level` is 1–6 and drives
    /// the emphasis ramp, not indentation — the pane is too narrow to indent.
    Heading {
        /// ATX heading level, 1 through 6.
        level: u8,
        /// The heading text, inline markup already flattened.
        text: String,
    },
    /// A markdown list item, rendered with a leading bullet glyph.
    Bullet(String),
    /// A line inside a fenced code block — verbatim, dimmed, never re-wrapped
    /// semantically (a command must stay copyable).
    Code(String),
    /// A markdown thematic break.
    Rule,
    /// A transient pane status (fetch in flight, fetch failed, nothing
    /// published). Dimmed, and never mistaken for artifact content.
    Notice(String),
}

/// Which tab of the detail pane is showing.
///
/// The set is **fixed**: every catalog row offers all three, whether or not its
/// repository published the documents behind them. An earlier revision showed
/// only the tabs with content, and that was the wrong trade — the strip changed
/// width as a fetch landed, `tab` did something different on every row, and
/// there was no way to learn the binding existed from a package that happened
/// to publish nothing. A tab with nothing behind it is greyed and says so when
/// you land on it, which is a stable, teachable shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DetailTab {
    /// Catalog metadata and support channels. Always has content, and is where
    /// every selection starts.
    #[default]
    Overview,
    /// The repository `README.md` from the description companion.
    Readme,
    /// The repository `CHANGELOG.md` from the description companion.
    Changelog,
}

impl DetailTab {
    /// Every tab, in strip order.
    pub const ALL: [Self; 3] = [Self::Overview, Self::Readme, Self::Changelog];

    /// The strip label.
    pub fn label(self) -> &'static str {
        match self {
            Self::Overview => "Overview",
            Self::Readme => "Readme",
            Self::Changelog => "Changelog",
        }
    }

    /// Whether `companion` has content behind this tab.
    ///
    /// Drives the greyed-out strip label only — never whether the tab can be
    /// selected. `Overview` is always live; the document tabs are live only
    /// once a fetch has landed and that document was published.
    pub fn is_live(self, companion: Option<&CompanionCache>) -> bool {
        match self {
            Self::Overview => true,
            Self::Readme => matches!(companion, Some(CompanionCache::Ready(c)) if c.readme.is_some()),
            Self::Changelog => matches!(companion, Some(CompanionCache::Ready(c)) if c.changelog.is_some()),
        }
    }
}

/// The detail pane's lines for `tab`.
///
/// Overview delegates to [`detail_lines`]; the document tabs render the
/// companion's markdown, or a [`DetailLine::Notice`] naming why there is none.
/// The notice is the normal case, not an edge case: every row offers all three
/// tabs, and most repositories publish neither document.
pub fn detail_tab_lines(row: Option<&TuiRow>, companion: Option<&CompanionCache>, tab: DetailTab) -> Vec<DetailLine> {
    // A leading blank so a notice does not sit flush against the block's top
    // border — the strip lives in the border now, and content butted straight
    // up against it reads cramped.
    // A `Failed` notice interpolates a transport error string, which can carry
    // a registry-supplied message — sanitize it like every other such string.
    let notice = |text: String| {
        vec![
            DetailLine::Blank,
            DetailLine::Notice(super::render::sanitize_member_label(&text)),
        ]
    };
    let document = |pick: fn(&Companion) -> Option<&String>| match companion {
        Some(CompanionCache::Ready(c)) => pick(c).map_or_else(
            || notice("not available".to_string()),
            |text| {
                let mut lines = vec![DetailLine::Blank];
                lines.extend(super::markdown::to_detail_lines(text));
                lines
            },
        ),
        Some(CompanionCache::Loading) => notice("loading…".to_string()),
        Some(CompanionCache::Failed(reason)) => notice(format!("not available — {reason}")),
        Some(CompanionCache::Absent) | None => notice("not available".to_string()),
    };
    match tab {
        DetailTab::Overview => detail_lines(row, companion),
        DetailTab::Readme => document(|c| c.readme.as_ref()),
        DetailTab::Changelog => document(|c| c.changelog.as_ref()),
    }
}

/// Build the Detail pane's semantic lines for the selected row.
///
/// Layout: the centered identifier framed by blank lines, an underlined
/// `Summary:` section (the short blurb, `-` when absent), an optional
/// `Description:` section (only when a description exists), then a
/// `Metadata:` section of `Label: value` rows. Version and status are
/// deliberately NOT repeated here — the catalog row already shows both
/// (Tag column, status glyph). `Pinned:` appears only when the picker
/// pinned a version.
pub fn detail_lines(row: Option<&TuiRow>, companion: Option<&CompanionCache>) -> Vec<DetailLine> {
    let Some(r) = row else {
        return vec![DetailLine::Text("no selection".to_string())];
    };
    let keywords = if r.keywords.is_empty() {
        "-".to_string()
    } else {
        r.keywords.join(", ")
    };
    let summary = if r.summary.is_empty() { "-" } else { r.summary.as_str() };

    let mut lines = vec![
        DetailLine::Blank,
        DetailLine::Identifier(r.repo.clone()),
        DetailLine::Blank,
        DetailLine::SectionLabel("Summary:"),
        DetailLine::Blank,
        DetailLine::Text(summary.to_string()),
    ];
    if !r.description.is_empty() {
        lines.extend([
            DetailLine::Blank,
            DetailLine::SectionLabel("Description:"),
            DetailLine::Blank,
            DetailLine::Text(r.description.clone()),
        ]);
    }
    lines.extend([
        DetailLine::Blank,
        DetailLine::SectionLabel("Metadata:"),
        DetailLine::Blank,
        DetailLine::MetaEntry {
            label: "Keywords:",
            value: keywords,
        },
        DetailLine::MetaEntry {
            label: "Repository:",
            value: r.repository_url.clone().unwrap_or_else(|| "-".to_string()),
        },
    ]);
    // A "Local" row (path declaration or dev record) carries no registry tag —
    // its `repository` field holds the declared source path and `version` the
    // short content hash, surfaced here as dedicated `Path:`/`Hash:` rows.
    if matches!(r.source, RowSource::Local) {
        lines.push(DetailLine::MetaEntry {
            label: "Path:",
            value: r.repository.clone(),
        });
        lines.push(DetailLine::MetaEntry {
            label: "Hash:",
            value: r.version.clone(),
        });
    }
    // Curated manifest metadata — each row shown only when the artifact
    // carries that annotation, so an artifact published without it keeps an
    // unchanged pane. All of these are version-scoped and reach the browse
    // catalog with the manifest. The repository's *support* channels do not:
    // they live on the mutable description companion, and the browse catalog
    // is disk-cached, so a pane fed from it would show a link that has since
    // moved. `grim describe` is the live surface for those.
    //
    // Every value here is an annotation written by whoever published the
    // artifact, so each is sanitized before it reaches a terminal cell. Until
    // this branch these five were dead read code — the write path emitted only
    // `licenses` — so populating them is what made the strip load-bearing.
    for (label, value) in [
        ("License:", &r.oci.licenses),
        ("Authors:", &r.oci.authors),
        ("URL:", &r.oci.url),
        ("Documentation:", &r.oci.documentation),
        ("Vendor:", &r.oci.vendor),
        ("Compatibility:", &r.oci.compatibility),
    ] {
        if let Some(value) = value {
            lines.push(DetailLine::MetaEntry {
                label,
                value: super::render::sanitize_member_label(value),
            });
        }
    }
    // Build provenance — derived by default from the publishing commit, so
    // most artifacts now carry it; still shown only when present, since
    // `--no-git` and a non-repository build both suppress it.
    if let Some(revision) = &r.revision {
        lines.push(DetailLine::MetaEntry {
            label: "Revision:",
            value: revision.clone(),
        });
    }
    if let Some(created) = &r.created {
        lines.push(DetailLine::MetaEntry {
            label: "Created:",
            value: created.clone(),
        });
    }
    // Community rating — shown only when the browse source published one, so
    // an unrated artifact's pane is unchanged. A count of 0 is never
    // synthesized for absence; the row simply is not there.
    if let Some(up) = r.rating {
        lines.push(DetailLine::MetaEntry {
            label: "Rating:",
            value: if up == 1 {
                "1 upvote".to_string()
            } else {
                format!("{up} upvotes")
            },
        });
    }
    if let Some(msg) = &r.deprecated {
        lines.push(DetailLine::MetaEntry {
            label: "Deprecated:",
            value: msg.clone(),
        });
    }
    if let Some(p) = &r.pinned_version {
        lines.push(DetailLine::MetaEntry {
            label: "Pinned:",
            value: p.clone(),
        });
    }
    // `integrity-missing` is the one badge whose cause is invisible from the
    // list, so it gets one static explanatory line. Static on purpose: the
    // state enum is matched at ~40 sites and gains no payload for this.
    if r.state == ArtifactState::IntegrityMissing {
        lines.push(DetailLine::MetaEntry {
            label: "Integrity:",
            value: "recorded files are missing, unreadable, or resolve outside their anchor root — \
                    uninstall and reinstall to repair"
                .to_string(),
        });
    }
    lines.extend(support_lines(companion));
    lines
}

/// The `Support:` section, from the live companion fetch.
///
/// Empty while a fetch is in flight and for a repository that published no
/// channels: a section that appears, says "loading…", and then either fills in
/// or vanishes would make the pane jump under the reader for metadata most
/// repositories do not publish at all.
///
/// A *failed* fetch is the exception and does render, because absence would
/// otherwise be ambiguous — see the body.
///
/// These channels are the one thing in this pane that does **not** come from
/// the browse catalog. They live on the mutable description companion, so a
/// disk-cached copy could show a contact link that has already moved — which is
/// exactly why `grim search` and the catalog row do not carry them
/// (`docs/src/publishing.md#metadata-surfaces`). Fetching them live on the
/// keypress that opens the pane is a different mechanism and carries no such
/// staleness.
fn support_lines(companion: Option<&CompanionCache>) -> Vec<DetailLine> {
    let c = match companion {
        Some(CompanionCache::Ready(c)) => c,
        // A failed fetch must say so *here*. Overview is where the channels
        // would have been, and silently omitting the section is
        // indistinguishable from a repository that publishes none — the reader
        // cannot tell "nobody to contact" from "we could not ask".
        Some(CompanionCache::Failed(reason)) => {
            return vec![
                DetailLine::Blank,
                DetailLine::SectionLabel("Support:"),
                DetailLine::Blank,
                DetailLine::Notice(format!("not available — {reason}")),
            ];
        }
        _ => return Vec::new(),
    };
    let channels = [
        ("Issues:", &c.support.issues),
        ("Chat:", &c.support.chat),
        ("Contact:", &c.support.contact),
        ("Security:", &c.support.security),
    ];
    let mut lines = Vec::new();
    for (label, value) in channels {
        if let Some(value) = value.as_deref().map(str::trim).filter(|v| !v.is_empty()) {
            lines.push(DetailLine::MetaEntry {
                label,
                // Publisher-controlled and headed for a terminal — same strip
                // the tree applies to every other registry-supplied string.
                value: super::render::sanitize_member_label(value),
            });
        }
    }
    if lines.is_empty() {
        return lines;
    }
    let mut section = vec![
        DetailLine::Blank,
        DetailLine::SectionLabel("Support:"),
        DetailLine::Blank,
    ];
    section.append(&mut lines);
    section
}

/// Build the Detail pane's semantic lines for a selected virtual bundle
/// member row.
///
/// # Contract (C-7)
///
/// Returns `[Identifier(sanitized label), Blank, SectionLabel("Metadata:"),
/// Blank, MetaEntry{Kind}, MetaEntry{State}, MetaEntry{"Via bundle:", parent_repo}]`.
///
/// Never reads `TuiRow` — `MemberNode` carries all the information needed.
/// The `label` is sanitized via `render::sanitize_member_label` before being
/// placed into the `Identifier` line.
///
/// `parent_bundle_repo` is the `registry/repository` of the bundle that owns
/// this member (from `DisplayRow::Member::parent_bundle_repo`); rendered as
/// the "Via bundle:" metadata line so the user can trace the virtual row back
/// to its parent.
pub fn detail_lines_for_member(node: &MemberNode, parent_bundle_repo: &str) -> Vec<DetailLine> {
    // Sanitize the raw label at the display boundary (never stored sanitized).
    let sanitized_label = super::render::sanitize_member_label(&node.label);

    // The identifier shown is the sanitized label. When a resolved `member_repo`
    // is available, prefer that for the canonical reference; fall back to the
    // sanitized label so the pane never shows an empty identifier.
    // Defense-in-depth: sanitize member_repo at the display boundary too —
    // even though it comes from Identifier::parse (charset-constrained), every
    // registry-derived string shown in the terminal passes through the sanitizer.
    let raw_identifier = node.member_repo.as_deref().unwrap_or(&sanitized_label);
    let identifier = super::render::sanitize_member_label(raw_identifier);

    vec![
        DetailLine::Blank,
        DetailLine::Identifier(identifier),
        DetailLine::Blank,
        DetailLine::SectionLabel("Metadata:"),
        DetailLine::Blank,
        DetailLine::MetaEntry {
            label: "Kind:",
            value: node.kind.to_string(),
        },
        DetailLine::MetaEntry {
            label: "State:",
            value: node.state.to_string(),
        },
        DetailLine::MetaEntry {
            label: "Via bundle:",
            // F9: sanitize at display boundary — parent_bundle_repo is
            // registry-controlled and must not reach the terminal raw.
            value: super::render::sanitize_member_label(parent_bundle_repo),
        },
    ]
}

/// The visible text of one semantic detail line (the wrap-count input;
/// tests reuse it to assert content without caring about styling).
pub fn detail_line_text(line: &DetailLine) -> String {
    match line {
        // A rule is painted as a full-width glyph run, which by construction
        // occupies exactly one row — measuring it as empty is what makes the
        // scroll bound agree with the paint.
        DetailLine::Blank | DetailLine::Rule => String::new(),
        DetailLine::Identifier(s) | DetailLine::Text(s) | DetailLine::Notice(s) => s.clone(),
        DetailLine::SectionLabel(l) => (*l).to_string(),
        DetailLine::MetaEntry { label, value } => format!("{label} {value}"),
        DetailLine::Heading { text, .. } => text.clone(),
        // The prefixes are part of the painted width, so they must be part of
        // the measured width too, or a bulleted list scrolls short.
        DetailLine::Bullet(s) => format!("{BULLET_PREFIX}{s}"),
        DetailLine::Code(s) => format!("{CODE_PREFIX}{s}"),
    }
}

/// Painted prefix of a [`DetailLine::Bullet`]. Shared by the wrap measurement
/// in [`detail_line_text`] and the draw in `render`, so the two cannot drift.
pub const BULLET_PREFIX: &str = "  • ";
/// Painted prefix of a [`DetailLine::Code`] line.
pub const CODE_PREFIX: &str = "    ";

/// The Detail pane's *inner* (border-less) size for a terminal of
/// `(width, height)` — mirrors the layout math in `render::draw`: 5 rows
/// of fixed chrome (title 1, search 3, legend 1), then side-by-side when
/// the catalog plus a usable detail column fit, else an even top/bottom
/// split of the content area (list on top, Detail below).
pub fn viewport(term: (u16, u16), show_registry_column: bool) -> (u16, u16) {
    let (w, h) = term;
    // The tab strip rides the block's own border, so it costs the body no row
    // and the pane's height does not change with what is selected.
    let content_h = h.saturating_sub(5);
    let catalog_w = catalog_width(show_registry_column);
    let (dw, dh) = if w >= catalog_w + DETAIL_MIN_WIDTH {
        (w - catalog_w, content_h)
    } else {
        // Stacked: list takes the floored half (`content_h / 2`), Detail the
        // remainder — matches the computed `Length(top)` split in render::draw.
        (w, content_h - content_h / 2)
    };
    (dw.saturating_sub(2), dh.saturating_sub(2))
}

/// Rows `text` occupies after greedy word-wrap at `width` columns —
/// the same strategy ratatui's `Wrap` uses (words longer than a row are
/// hard-broken). An empty line still occupies one row.
fn wrapped_rows(text: &str, width: u16) -> usize {
    let width = usize::from(width.max(1));
    let mut rows = 1usize;
    let mut col = 0usize;
    for word in text.split_whitespace() {
        let len = word.chars().count();
        // A separating space is needed when the row already has content.
        let needed = if col == 0 { len } else { len + 1 };
        if col + needed <= width {
            col += needed;
        } else if len <= width {
            rows += 1;
            col = len;
        } else {
            // Longer than a full row: hard-broken across rows.
            if col > 0 {
                rows += 1;
            }
            let mut remaining = len;
            while remaining > width {
                rows += 1;
                remaining -= width;
            }
            col = remaining;
        }
    }
    rows
}

/// Upper bound for the detail scroll offset: the content's post-wrap row
/// count minus the viewport height, so the content's last row stops at
/// the pane's bottom edge (no scrolling into blank space). Zero when the
/// content fits the pane.
pub fn scroll_max(lines: &[DetailLine], viewport: (u16, u16)) -> u16 {
    let (vw, vh) = viewport;
    let rows: usize = lines.iter().map(|l| wrapped_rows(&detail_line_text(l), vw)).sum();
    u16::try_from(rows.saturating_sub(usize::from(vh))).unwrap_or(u16::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── C-7 detail_lines_for_member ───────────────────────────────────────────
    //
    // These tests FAIL until P3 implements `detail_lines_for_member`.

    use crate::oci::ArtifactKind;
    use crate::tui::bundle_members::MemberNode;
    use crate::tui::state::ArtifactState;

    fn make_member_node(
        label: &str,
        kind: ArtifactKind,
        member_repo: Option<&str>,
        state: ArtifactState,
    ) -> MemberNode {
        MemberNode {
            kind,
            label: label.to_string(),
            member_repo: member_repo.map(|s| s.to_string()),
            state,
            related: false,
        }
    }

    #[test]
    fn detail_lines_for_member_returns_identifier_and_metadata_entries() {
        // C-7: detail pane for a Member must include Identifier (sanitized label),
        // and MetaEntry rows for Kind, State, and "Via bundle:" parent repo.
        // Layout: [Identifier(sanitized), Blank, SectionLabel("Metadata:"), Blank,
        //          MetaEntry{Kind}, MetaEntry{State}, MetaEntry{"Via bundle:", parent_repo}]
        let node = MemberNode {
            kind: ArtifactKind::Skill,
            label: "my-skill".to_string(),
            member_repo: Some("reg/acme/my-skill".to_string()),
            state: ArtifactState::Installed,
            related: false,
        };
        // Per C-7, parent_repo comes from DisplayRow::Member.parent_bundle_repo
        // and is threaded in by the call site.
        let parent_repo = "reg.example.io/acme/my-bundle";
        let lines = detail_lines_for_member(&node, parent_repo);
        // Must be non-empty.
        assert!(!lines.is_empty(), "C-7: detail lines for member must be non-empty");
        // Must have an Identifier line (the sanitized label).
        let has_identifier = lines.iter().any(|l| matches!(l, DetailLine::Identifier(_)));
        assert!(
            has_identifier,
            "C-7: must include a Identifier line for the member label"
        );
        // Must have "Metadata:" section label.
        let has_metadata = lines.iter().any(|l| matches!(l, DetailLine::SectionLabel("Metadata:")));
        assert!(has_metadata, "C-7: must include a Metadata: section label");
        // Must have a MetaEntry for Kind.
        let has_kind = lines
            .iter()
            .any(|l| matches!(l, DetailLine::MetaEntry { label: "Kind:", .. }));
        assert!(has_kind, "C-7: must include MetaEntry{{Kind:}}");
        // Must have a MetaEntry for State.
        let has_state = lines
            .iter()
            .any(|l| matches!(l, DetailLine::MetaEntry { label: "State:", .. }));
        assert!(has_state, "C-7: must include MetaEntry{{State:}}");
        // Must have a MetaEntry for "Via bundle:" with the parent repo value (B2 assertion).
        let via_bundle = lines.iter().find_map(|l| match l {
            DetailLine::MetaEntry {
                label: "Via bundle:",
                value,
            } => Some(value.clone()),
            _ => None,
        });
        assert!(
            via_bundle.is_some(),
            "C-7: must include MetaEntry{{\"Via bundle:\", ...}} line"
        );
        assert_eq!(
            via_bundle.as_deref(),
            Some(parent_repo),
            "C-7: Via bundle: value must be the parent bundle repo"
        );
    }

    #[test]
    fn detail_lines_for_member_never_reads_tui_row() {
        // C-7 invariant: the function operates only on MemberNode, never a TuiRow.
        // This is a structural test — if the function compiled and returns lines
        // from a MemberNode alone, it proves TuiRow independence. We verify it
        // produces output for a "minimal" MemberNode (rule kind, no member_repo).
        let node = make_member_node("some-rule", ArtifactKind::Rule, None, ArtifactState::NotInstalled);
        let lines = detail_lines_for_member(&node, "reg/acme/test-bundle");
        assert!(
            !lines.is_empty(),
            "C-7: even a rule member with no repo must produce lines"
        );
        let has_identifier = lines.iter().any(|l| matches!(l, DetailLine::Identifier(_)));
        assert!(has_identifier, "C-7: rule member must include an Identifier line");
    }

    #[test]
    fn detail_lines_for_member_with_unparseable_id_still_renders() {
        // C-7 edge case: `member_repo = None` (Identifier::parse failed, fail-soft).
        // The node must still render — nothing panics, non-empty output.
        let node = make_member_node(
            "bad-id:://invalid",
            ArtifactKind::Skill,
            None,
            ArtifactState::NotInstalled,
        );
        let lines = detail_lines_for_member(&node, "reg/acme/test-bundle");
        assert!(
            !lines.is_empty(),
            "C-7: unparseable-id member (member_repo=None) must still render, got empty"
        );
    }

    #[test]
    fn detail_lines_for_member_label_appears_in_identifier_line() {
        // C-7: the Identifier line value must contain the (sanitized) label.
        let node = make_member_node(
            "code-review",
            ArtifactKind::Skill,
            Some("reg/acme/code-review"),
            ArtifactState::Installed,
        );
        let lines = detail_lines_for_member(&node, "reg/acme/test-bundle");
        let identifier_value = lines.iter().find_map(|l| match l {
            DetailLine::Identifier(s) => Some(s.clone()),
            _ => None,
        });
        assert!(identifier_value.is_some(), "C-7: must have an Identifier line");
        assert!(
            identifier_value.as_ref().unwrap().contains("code-review"),
            "C-7: Identifier line must contain the label; got: {:?}",
            identifier_value
        );
    }

    fn tui_row(deprecated: Option<&str>) -> TuiRow {
        TuiRow {
            oci: crate::catalog::OciMeta::default(),
            kind: "skill".to_string(),
            registry: "localhost:5000".to_string(),
            repository: "acme/code-review".to_string(),
            repo: "localhost:5000/acme/code-review".to_string(),
            description: String::new(),
            summary: "blurb".to_string(),
            keywords: vec![],
            repository_url: None,
            revision: None,
            created: None,
            rating: None,
            deprecated: deprecated.map(str::to_string),
            latest_tag: "1.0.0".to_string(),
            version: "1.0.0".to_string(),
            pinned_version: None,
            state: ArtifactState::NotInstalled,
            source: RowSource::Unattributed,
        }
    }

    // ── Tabs and the live companion ──────────────────────────────────────

    fn companion(readme: Option<&str>, changelog: Option<&str>) -> CompanionCache {
        CompanionCache::Ready(Box::new(Companion {
            support: Default::default(),
            readme: readme.map(str::to_string),
            changelog: changelog.map(str::to_string),
        }))
    }

    #[test]
    fn every_tab_is_greyed_but_present_without_a_companion() {
        // The whole no-companion path: no fetch yet, a fetch in flight, a
        // failed fetch, and a repository that genuinely publishes nothing must
        // all agree — Overview live, the document tabs present but empty. The
        // strip must NOT change membership as a fetch lands, or the pane
        // resizes under the reader and `tab` means something different on
        // every row.
        for state in [
            None,
            Some(&CompanionCache::Loading),
            Some(&CompanionCache::Absent),
            Some(&CompanionCache::Failed("offline".to_string())),
        ] {
            assert!(DetailTab::Overview.is_live(state), "{state:?}");
            assert!(!DetailTab::Readme.is_live(state), "{state:?}");
            assert!(!DetailTab::Changelog.is_live(state), "{state:?}");
        }
    }

    #[test]
    fn a_published_document_makes_exactly_its_own_tab_live() {
        let readme_only = companion(Some("# r"), None);
        assert!(DetailTab::Readme.is_live(Some(&readme_only)));
        assert!(!DetailTab::Changelog.is_live(Some(&readme_only)));

        let changelog_only = companion(None, Some("## 1.0.0"));
        assert!(!DetailTab::Readme.is_live(Some(&changelog_only)));
        assert!(DetailTab::Changelog.is_live(Some(&changelog_only)));

        let both = companion(Some("# r"), Some("## 1.0.0"));
        assert!(DetailTab::Readme.is_live(Some(&both)));
        assert!(DetailTab::Changelog.is_live(Some(&both)));
    }

    #[test]
    fn support_channels_alone_leave_both_document_tabs_empty() {
        // Support channels are worth caching as `Ready` — they render in
        // Overview — but they are not a document, so neither tab lights up.
        let support_only = CompanionCache::Ready(Box::new(Companion {
            support: crate::oci::description::SupportLinks {
                issues: Some("https://example.invalid/issues".to_string()),
                ..Default::default()
            },
            ..Default::default()
        }));
        assert!(!DetailTab::Readme.is_live(Some(&support_only)));
        assert!(!DetailTab::Changelog.is_live(Some(&support_only)));
    }

    #[test]
    fn a_document_tab_renders_the_companion_markdown() {
        let row = tui_row(None);
        let cache = companion(Some("# Title\n\ntext"), None);
        let lines = detail_tab_lines(Some(&row), Some(&cache), DetailTab::Readme);
        assert_eq!(
            lines,
            vec![
                // A leading blank keeps the body off the block's top border,
                // which now carries the tab strip.
                DetailLine::Blank,
                DetailLine::Heading {
                    level: 1,
                    text: "Title".to_string()
                },
                DetailLine::Blank,
                DetailLine::Text("text".to_string()),
            ]
        );
    }

    #[test]
    fn a_document_tab_without_content_says_why_instead_of_rendering_empty() {
        let row = tui_row(None);
        for (cache, needle) in [
            (Some(CompanionCache::Loading), "loading"),
            (Some(CompanionCache::Absent), "not available"),
            (Some(CompanionCache::Failed("offline".to_string())), "offline"),
            (None, "not available"),
        ] {
            let lines = detail_tab_lines(Some(&row), cache.as_ref(), DetailTab::Readme);
            assert_eq!(lines[0], DetailLine::Blank, "the notice must clear the top border");
            let DetailLine::Notice(text) = &lines[1] else {
                panic!("expected a notice, got {lines:?}");
            };
            assert!(text.contains(needle), "{text:?} must mention {needle:?}");
        }
    }

    #[test]
    fn overview_gains_a_support_section_only_from_a_ready_companion() {
        let row = tui_row(None);
        let with_channels = CompanionCache::Ready(Box::new(Companion {
            support: crate::oci::description::SupportLinks {
                issues: Some("https://example.invalid/issues".to_string()),
                // A blank authored value is not a channel — the same
                // trim/empty-is-absent rule the publish side applies.
                chat: Some("   ".to_string()),
                ..Default::default()
            },
            ..Default::default()
        }));
        let lines = detail_lines(Some(&row), Some(&with_channels));
        assert_eq!(meta_value(&lines, "Issues:"), Some("https://example.invalid/issues"));
        assert_eq!(meta_value(&lines, "Chat:"), None, "a blank value is not a channel");
        assert!(lines.contains(&DetailLine::SectionLabel("Support:")));
    }

    #[test]
    fn overview_shows_no_support_section_while_the_fetch_is_pending_or_absent() {
        // A section that appears, says "loading…", then vanishes would make the
        // pane jump under the reader for metadata most repositories never
        // publish. Absence is the honest resting state.
        let row = tui_row(None);
        for state in [None, Some(&CompanionCache::Loading), Some(&CompanionCache::Absent)] {
            let lines = detail_lines(Some(&row), state);
            assert!(
                !lines.contains(&DetailLine::SectionLabel("Support:")),
                "{state:?} must not open a Support section"
            );
        }
    }

    #[test]
    fn overview_says_so_when_the_support_fetch_failed() {
        // The one case that must NOT be silent: an omitted section is
        // indistinguishable from a repository that publishes no channels, so
        // the reader cannot tell "nobody to contact" from "we could not ask".
        let row = tui_row(None);
        let lines = detail_lines(Some(&row), Some(&CompanionCache::Failed("offline".to_string())));
        assert!(lines.contains(&DetailLine::SectionLabel("Support:")));
        assert!(
            lines
                .iter()
                .any(|l| matches!(l, DetailLine::Notice(t) if t.contains("offline"))),
            "the cause must reach the pane: {lines:?}"
        );
    }

    #[test]
    fn the_tab_strip_costs_the_body_nothing() {
        // The strip rides the block's border, so the pane's height is the same
        // whether or not it is painted. A body that shrank when a strip
        // appeared made the pane resize under the reader.
        let term = (CATALOG_WIDTH + 60, 30);
        assert_eq!(viewport(term, false), viewport(term, false));
        let (_, h) = viewport(term, false);
        assert_eq!(h, 30 - 5 - 2, "chrome (5) and the block's own borders (2)");
    }

    #[test]
    fn detail_lines_show_deprecated_meta_entry_when_deprecated() {
        let lines = detail_lines(Some(&tui_row(Some("use acme/code-review-2"))), None);
        let dep = lines.iter().find_map(|l| match l {
            DetailLine::MetaEntry {
                label: "Deprecated:",
                value,
            } => Some(value.clone()),
            _ => None,
        });
        assert_eq!(dep.as_deref(), Some("use acme/code-review-2"));
    }

    /// Collect the value of the `MetaEntry` with the given label, if present.
    fn meta_value<'a>(lines: &'a [DetailLine], want: &str) -> Option<&'a str> {
        lines.iter().find_map(|l| match l {
            DetailLine::MetaEntry { label, value } if *label == want => Some(value.as_str()),
            _ => None,
        })
    }

    #[test]
    fn detail_lines_show_curated_oci_metadata_when_present() {
        let mut row = tui_row(None);
        row.oci = crate::catalog::OciMeta {
            licenses: Some("Apache-2.0".to_string()),
            authors: Some("Jane Doe".to_string()),
            url: Some("https://acme.example".to_string()),
            documentation: Some("https://docs.acme.example".to_string()),
            vendor: Some("Acme Inc".to_string()),
            compatibility: Some("claude>=2".to_string()),
        };
        let lines = detail_lines(Some(&row), None);
        assert_eq!(meta_value(&lines, "License:"), Some("Apache-2.0"));
        assert_eq!(meta_value(&lines, "Authors:"), Some("Jane Doe"));
        assert_eq!(meta_value(&lines, "URL:"), Some("https://acme.example"));
        assert_eq!(meta_value(&lines, "Documentation:"), Some("https://docs.acme.example"));
        assert_eq!(meta_value(&lines, "Vendor:"), Some("Acme Inc"));
        assert_eq!(meta_value(&lines, "Compatibility:"), Some("claude>=2"));
    }

    #[test]
    fn detail_lines_omit_curated_oci_metadata_when_absent() {
        // A default (empty) OciMeta shows none of the curated rows.
        let lines = detail_lines(Some(&tui_row(None)), None);
        for label in [
            "License:",
            "Authors:",
            "URL:",
            "Documentation:",
            "Vendor:",
            "Compatibility:",
        ] {
            assert_eq!(meta_value(&lines, label), None, "{label} must be absent when unset");
        }
    }

    #[test]
    fn detail_lines_show_only_the_present_oci_fields() {
        // Only `licenses` set ⇒ only the License row appears (partial metadata).
        let mut row = tui_row(None);
        row.oci.licenses = Some("MIT".to_string());
        let lines = detail_lines(Some(&row), None);
        assert_eq!(meta_value(&lines, "License:"), Some("MIT"));
        for label in ["Authors:", "URL:", "Documentation:", "Vendor:", "Compatibility:"] {
            assert_eq!(meta_value(&lines, label), None);
        }
    }

    /// A7 / W3. `integrity-missing` is the one badge whose cause is invisible
    /// from the list, so the detail pane carries the only explanation the user
    /// gets — including for the containment refusal, whose remediation
    /// (uninstall + reinstall) appears nowhere else on this row.
    #[test]
    fn detail_lines_explain_an_integrity_missing_row() {
        let mut row = tui_row(None);
        row.state = ArtifactState::IntegrityMissing;
        let lines = detail_lines(Some(&row), None);
        let integrity = meta_value(&lines, "Integrity:").expect("integrity-missing gets its line");
        assert!(integrity.contains("outside their anchor root"), "got {integrity}");
        assert!(integrity.contains("uninstall and reinstall"), "got {integrity}");
        // D3: a containment refusal is never offered an override control.
        assert!(
            !integrity.contains("force"),
            "the explanation must not suggest an override: {integrity}"
        );
    }

    #[test]
    fn detail_lines_omit_the_integrity_line_for_every_other_state() {
        // Only the one badge earns the line — an ordinary row's pane is
        // unchanged.
        for state in [
            ArtifactState::Installed,
            ArtifactState::NotInstalled,
            ArtifactState::Modified,
            ArtifactState::Outdated,
            ArtifactState::ViaBundle,
        ] {
            let mut row = tui_row(None);
            row.state = state;
            assert_eq!(
                meta_value(&detail_lines(Some(&row), None), "Integrity:"),
                None,
                "{state:?} must not carry the Integrity line"
            );
        }
    }

    #[test]
    fn detail_lines_show_git_provenance_when_present() {
        let mut row = tui_row(None);
        row.revision = Some("abc123def456-dirty".to_string());
        row.created = Some("2026-06-29T12:00:00+00:00".to_string());
        let lines = detail_lines(Some(&row), None);
        let revision = lines.iter().find_map(|l| match l {
            DetailLine::MetaEntry {
                label: "Revision:",
                value,
            } => Some(value.clone()),
            _ => None,
        });
        assert_eq!(revision.as_deref(), Some("abc123def456-dirty"));
        let created = lines.iter().find_map(|l| match l {
            DetailLine::MetaEntry {
                label: "Created:",
                value,
            } => Some(value.clone()),
            _ => None,
        });
        assert_eq!(created.as_deref(), Some("2026-06-29T12:00:00+00:00"));
    }

    #[test]
    fn detail_lines_omit_git_provenance_when_absent() {
        // An artifact published without `--git` shows neither row.
        let lines = detail_lines(Some(&tui_row(None)), None);
        assert!(!lines.iter().any(|l| matches!(
            l,
            DetailLine::MetaEntry { label: "Revision:", .. } | DetailLine::MetaEntry { label: "Created:", .. }
        )));
    }

    #[test]
    fn detail_lines_show_rating_beside_git_provenance_when_present() {
        let mut row = tui_row(None);
        row.revision = Some("abc123def456".to_string());
        row.created = Some("2026-06-29T12:00:00+00:00".to_string());
        row.rating = Some(42);
        let lines = detail_lines(Some(&row), None);
        assert_eq!(meta_value(&lines, "Rating:"), Some("42 upvotes"));
        // Beside `Revision:`/`Created:`, not before them.
        let pos = |want: &str| {
            lines
                .iter()
                .position(|l| matches!(l, DetailLine::MetaEntry { label, .. } if *label == want))
        };
        assert!(pos("Created:") < pos("Rating:"), "Rating follows the provenance rows");
        // A single vote reads grammatically.
        row.rating = Some(1);
        assert_eq!(meta_value(&detail_lines(Some(&row), None), "Rating:"), Some("1 upvote"));
    }

    #[test]
    fn detail_lines_omit_rating_when_unrated() {
        // Absent ⇒ no row at all. A `0` here would read as "rated, nobody
        // voted", which is a different fact than "we have no rating".
        let row = tui_row(None);
        assert_eq!(row.rating, None, "the fixture is unrated");
        let lines = detail_lines(Some(&row), None);
        assert_eq!(meta_value(&lines, "Rating:"), None);
        assert!(
            !lines
                .iter()
                .any(|l| matches!(l, DetailLine::MetaEntry { value, .. } if value == "0")),
            "no zero-valued meta entry is synthesized for an unrated row"
        );
    }

    #[test]
    fn detail_lines_omit_deprecated_meta_entry_when_not_deprecated() {
        let lines = detail_lines(Some(&tui_row(None)), None);
        assert!(
            !lines.iter().any(|l| matches!(
                l,
                DetailLine::MetaEntry {
                    label: "Deprecated:",
                    ..
                }
            )),
            "a non-deprecated row must not show a Deprecated: entry"
        );
    }

    // Design record: local_bundles_tui_group plan, "TUI Local group" — a
    // "Local" row (path declaration or dev record) carries no registry tag;
    // the detail pane shows its local source path and short content hash
    // via dedicated `Path:`/`Hash:` rows instead.
    #[test]
    fn detail_lines_show_path_and_hash_for_local_row() {
        let mut row = tui_row(None);
        row.source = RowSource::Local;
        row.repository = "./local-skill".to_string();
        row.version = "deadbee1".to_string();
        let lines = detail_lines(Some(&row), None);
        assert_eq!(meta_value(&lines, "Path:"), Some("./local-skill"));
        assert_eq!(meta_value(&lines, "Hash:"), Some("deadbee1"));
    }

    #[test]
    fn detail_lines_omit_path_and_hash_for_registry_row() {
        // A registry-sourced row (`RowSource::Unattributed`) must never show the
        // Local-only Path:/Hash: rows.
        let lines = detail_lines(Some(&tui_row(None)), None);
        assert_eq!(meta_value(&lines, "Path:"), None);
        assert_eq!(meta_value(&lines, "Hash:"), None);
    }

    #[test]
    fn wrapped_rows_counts_word_wrap() {
        // Empty and short lines occupy one row.
        assert_eq!(wrapped_rows("", 10), 1);
        assert_eq!(wrapped_rows("abc def", 10), 1);
        // Exact fit stays one row; one char over wraps.
        assert_eq!(wrapped_rows("abcd efghi", 10), 1);
        assert_eq!(wrapped_rows("abcde fghij", 10), 2);
        // Words pack greedily with single separating spaces.
        assert_eq!(wrapped_rows("aa bb cc dd", 5), 2);
        assert_eq!(wrapped_rows("aaa bbb ccc", 5), 3);
        // A word longer than the row is hard-broken like ratatui does.
        assert_eq!(wrapped_rows(&"x".repeat(25), 10), 3);
        // …also when it follows content on the current row.
        assert_eq!(wrapped_rows(&format!("ab {}", "x".repeat(25)), 10), 4);
        // Degenerate width never divides by zero.
        assert_eq!(wrapped_rows("ab", 0), 2);
    }

    #[test]
    fn viewport_mirrors_the_layout_split() {
        // Wide: side-by-side — detail gets all slack minus borders.
        let (w, h) = viewport((CATALOG_WIDTH + 60, 30), false);
        assert_eq!((w, h), (58, 23));
        // Narrow: stacked — full width, Detail gets the top-half remainder.
        // content_h = 25, list = 12, Detail = 13, inner = 11.
        let (w, h) = viewport((80, 30), false);
        assert_eq!((w, h), (78, 11));
        // Short terminal: content_h = 5, Detail = 3, inner = 1.
        let (_, h) = viewport((80, 10), false);
        assert_eq!(h, 1);
        // Tiny: saturates, never underflows.
        assert_eq!(viewport((0, 0), false), (0, 0));
    }

    #[test]
    fn viewport_reserves_the_registry_column_when_shown() {
        // Multi-registry flat view: the Catalog claims `W_REGISTRY + 2` more
        // columns, so the Detail pane at the same terminal width is narrower
        // than the single-registry case by exactly that amount.
        let term = (catalog_width(true) + 60, 30);
        let (w_multi, _) = viewport(term, true);
        let (w_single, _) = viewport(term, false);
        assert_eq!(w_single - w_multi, (W_REGISTRY + 2) as u16);
        // Detail still gets the slack beyond the wider Catalog: 60 - borders.
        assert_eq!(w_multi, 58);
    }

    #[test]
    fn scroll_max_stops_at_the_content_end() {
        let lines: Vec<DetailLine> = (0..10).map(|i| DetailLine::Text(format!("line {i}"))).collect();
        // Content taller than the pane: last row aligns with the bottom.
        assert_eq!(scroll_max(&lines, (40, 4)), 6);
        // Content fits: no scrolling at all.
        assert_eq!(scroll_max(&lines, (40, 10)), 0);
        assert_eq!(scroll_max(&lines, (40, 20)), 0);
        // Wrapping at a narrow pane raises the row count.
        let long = vec![DetailLine::Text("a".repeat(100))];
        assert_eq!(scroll_max(&long, (10, 4)), 6);
    }
}
