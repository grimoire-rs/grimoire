// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! Format-preserving rewrite of `grimoire.toml`: [`merge`] folds a rendered
//! config into the text on disk through [`toml_edit`], so only the lines whose
//! meaning changed move.
//!
//! - an unchanged value stays as written (a declaration compares by source);
//! - a changed value keeps its position and its trailing comment;
//! - a new key appends to its table, a new table takes its canonical slot;
//! - a removed key's own comment goes, a blank-line-separated one moves on;
//! - an absent `false`/`[]` key stays; an emptied table goes unless commented.

use toml_edit::{ArrayOfTables, DocumentMut, Item, Table, TableLike, TomlError, Value};

use crate::config::declaration::DeclaredSource;
use crate::config::path_source::{PathSource, is_path_value};
use crate::install::toml_splice::toml_item_to_json;
use crate::oci::Identifier;

/// Top-level tables whose leaves are artifact declarations.
const DECLARATION_TABLES: [&str; 6] = ["bundles", "skills", "rules", "agents", "mcp", "hooks"];

/// Merge `desired` (a rendered config) into `existing` (the file on disk).
///
/// # Errors
///
/// Either text is not valid TOML.
pub(crate) fn merge(existing: &str, desired: &str) -> Result<String, TomlError> {
    let mut doc: DocumentMut = existing.parse()?;
    let want: DocumentMut = desired.parse()?;
    let mut added = Vec::new();
    merge_table(doc.as_table_mut(), want.as_table(), &mut Vec::new(), &mut added);
    let order = table_order(want.as_table());
    for path in added {
        place(&mut doc, &path, &order);
    }
    Ok(doc.to_string())
}

/// Where a table was added: its key path, plus the array index for an
/// array-of-tables element.
struct Added {
    keys: Vec<String>,
    index: Option<usize>,
}

fn merge_table(have: &mut dyn TableLike, want: &dyn TableLike, path: &mut Vec<String>, added: &mut Vec<Added>) {
    let have_keys: Vec<String> = have.iter().map(|(k, _)| k.to_owned()).collect();
    for key in have_keys.iter().filter(|k| !want.contains_key(k)) {
        let Some(item) = have.get_mut(key) else { continue };
        if let Some(table) = item.as_table_like_mut() {
            path.push(key.clone());
            merge_table(table, &Table::new(), path, added);
            path.pop();
            if prunable(item) {
                have.remove(key);
            }
        } else if !item.as_value().is_some_and(reads_as_default) {
            remove_entry(have, key);
        }
    }

    for (key, w) in want.iter() {
        path.push(key.to_owned());
        match have.get_mut(key) {
            Some(h) => merge_item(h, w, path, added),
            None if is_empty_table(w) => {}
            None => {
                have.insert(key, fresh(w, path, added));
            }
        }
        path.pop();
    }
}

fn merge_item(have: &mut Item, want: &Item, path: &mut Vec<String>, added: &mut Vec<Added>) {
    match (have, want) {
        (Item::ArrayOfTables(h), Item::ArrayOfTables(w)) => merge_array(h, w, path, added),
        (h, w) if h.is_table_like() && w.is_table_like() => {
            let (Some(ht), Some(wt)) = (h.as_table_like_mut(), w.as_table_like()) else {
                return;
            };
            let before = ht.len();
            merge_table(ht, wt, path, added);
            // An appended key inherits the old last key's spacing before `}`;
            // an inline table is one line, so renormalize it.
            if let Item::Value(Value::InlineTable(t)) = h
                && t.len() > before
            {
                t.fmt();
            }
            // A header-less parent (`[options.tui]` with no `[options]`) that
            // now carries its own keys must print a header for them.
            if let Item::Table(t) = h
                && t.is_implicit()
                && t.iter().any(|(_, i)| i.is_value())
            {
                t.set_implicit(false);
                added.push(Added {
                    keys: path.clone(),
                    index: None,
                });
            }
        }
        (Item::Value(h), Item::Value(w)) => {
            if !same_value(h, w, path) {
                let decor = h.decor().clone();
                *h = w.clone();
                *h.decor_mut() = decor;
            }
        }
        (h, w) => *h = fresh(w, path, added),
    }
}

/// Registries match by `alias`, else by locator, so an entry keeps its
/// comments when another one is added, edited or removed around it.
fn merge_array(have: &mut ArrayOfTables, want: &ArrayOfTables, path: &mut Vec<String>, added: &mut Vec<Added>) {
    let mut old: Vec<Option<Table>> = std::mem::take(have).into_iter().map(Some).collect();
    for (index, w) in want.iter().enumerate() {
        let hit = old
            .iter()
            .position(|o| o.as_ref().is_some_and(|o| same_identity(o, w)))
            .and_then(|i| old[i].take());
        let table = match hit {
            Some(mut t) => {
                merge_table(&mut t, w, path, added);
                t
            }
            None => {
                let mut t = w.clone();
                clear_positions(&mut t);
                added.push(Added {
                    keys: path.clone(),
                    index: Some(index),
                });
                t
            }
        };
        have.push(table);
    }
}

fn same_identity(have: &Table, want: &Table) -> bool {
    let key = ["alias", "oci", "index"]
        .into_iter()
        .find(|k| want.contains_key(k))
        .unwrap_or("alias");
    match (have.get(key), want.get(key)) {
        (Some(h), Some(w)) => toml_item_to_json(h) == toml_item_to_json(w),
        _ => false,
    }
}

fn same_value(have: &Value, want: &Value, path: &[String]) -> bool {
    if path.len() == 2
        && DECLARATION_TABLES.contains(&path[0].as_str())
        && let (Some(h), Some(w)) = (have.as_str(), want.as_str())
    {
        return h == w || declared(h).is_some_and(|h| declared(w) == Some(h));
    }
    toml_item_to_json(&Item::Value(have.clone())) == toml_item_to_json(&Item::Value(want.clone()))
}

/// A declaration value as the config loader reads it (`parse_artifact_map`).
/// `None` for a value it would reject, which then compares unequal and is
/// rewritten — the safe direction.
fn declared(raw: &str) -> Option<DeclaredSource> {
    if is_path_value(raw) {
        return PathSource::parse(raw).ok().map(DeclaredSource::Path);
    }
    Identifier::parse(raw)
        .ok()
        .map(|id| DeclaredSource::Registry(id.or_latest()))
}

fn reads_as_default(value: &Value) -> bool {
    match value {
        Value::Boolean(b) => !*b.value(),
        Value::Array(a) => a.is_empty(),
        _ => false,
    }
}

fn is_empty_table(item: &Item) -> bool {
    item.as_table_like().is_some_and(|t| t.is_empty())
}

fn prunable(item: &Item) -> bool {
    let commented = match item {
        Item::Table(t) => has_comment(t.decor().prefix()),
        _ => false,
    };
    is_empty_table(item) && !commented
}

fn has_comment(raw: Option<&toml_edit::RawString>) -> bool {
    raw.and_then(|r| r.as_str()).is_some_and(|s| s.contains('#'))
}

/// A desired item copied in as new: positions cleared so [`place`] sets
/// them, and a table holding only sub-tables stays header-less.
fn fresh(want: &Item, path: &[String], added: &mut Vec<Added>) -> Item {
    let mut item = want.clone();
    match &mut item {
        Item::Table(t) => {
            clear_positions(t);
            if !t.iter().any(|(_, i)| i.is_value()) {
                t.set_implicit(true);
            }
            added.push(Added {
                keys: path.to_vec(),
                index: None,
            });
        }
        Item::ArrayOfTables(a) => {
            for (index, t) in a.iter_mut().enumerate() {
                clear_positions(t);
                added.push(Added {
                    keys: path.to_vec(),
                    index: Some(index),
                });
            }
        }
        _ => {}
    }
    item
}

/// Remove `key`, moving the part of its leading comment that a blank line
/// sets off from it (a section comment) onto the next value key.
fn remove_entry(table: &mut dyn TableLike, key: &str) {
    let prefix = table
        .key(key)
        .and_then(|k| k.leaf_decor().prefix())
        .and_then(|r| r.as_str())
        .map(str::to_owned);
    let keys: Vec<String> = table.iter().map(|(k, _)| k.to_owned()).collect();
    let next = keys
        .iter()
        .skip_while(|k| k.as_str() != key)
        .nth(1)
        .filter(|k| table.get(k).is_some_and(Item::is_value))
        .cloned();
    table.remove(key);
    let (Some(section), Some(next)) = (prefix.as_deref().and_then(section_comment), next) else {
        return;
    };
    let Some(mut next_key) = table.key_mut(&next) else {
        return;
    };
    let decor = next_key.leaf_decor_mut();
    let own = decor.prefix().and_then(|r| r.as_str()).unwrap_or("").to_owned();
    // The section already ends on a blank line; a second one would double it.
    let own = own.strip_prefix('\n').unwrap_or(&own);
    decor.set_prefix(format!("{section}{own}"));
}

/// The leading part of `prefix` up to and including its last blank line.
fn section_comment(prefix: &str) -> Option<&str> {
    let mut end = None;
    let mut offset = 0;
    for line in prefix.split_inclusive('\n') {
        offset += line.len();
        if line.trim().is_empty() && line.ends_with('\n') {
            end = Some(offset);
        }
    }
    end.map(|e| &prefix[..e]).filter(|s| !s.is_empty())
}

fn clear_positions(table: &mut Table) {
    table.set_position(None);
    for (_, item) in table.iter_mut() {
        match item {
            Item::Table(t) => clear_positions(t),
            Item::ArrayOfTables(a) => a.iter_mut().for_each(clear_positions),
            _ => {}
        }
    }
}

/// Every header path of `root` in render order (positions, as the encoder
/// sorts them), each listed once.
fn table_order(root: &Table) -> Vec<Vec<String>> {
    let mut found = Vec::new();
    collect_tables(root, &mut Vec::new(), &mut found);
    found.sort_by_key(|(pos, _)| *pos);
    let mut order: Vec<Vec<String>> = Vec::new();
    for (_, path) in found {
        if !order.contains(&path) {
            order.push(path);
        }
    }
    order
}

fn collect_tables(table: &Table, path: &mut Vec<String>, out: &mut Vec<(isize, Vec<String>)>) {
    for (key, item) in table.iter() {
        path.push(key.to_owned());
        match item {
            Item::Table(t) => {
                if let Some(pos) = t.position() {
                    out.push((pos, path.clone()));
                }
                collect_tables(t, path, out);
            }
            Item::ArrayOfTables(a) => {
                for t in a.iter() {
                    if let Some(pos) = t.position() {
                        out.push((pos, path.clone()));
                    }
                    collect_tables(t, path, out);
                }
            }
            _ => {}
        }
        path.pop();
    }
}

/// Give an added table its document position: after the previous element
/// of its array, else before the first existing table that follows it in
/// `order`, else last.
fn place(doc: &mut DocumentMut, added: &Added, order: &[Vec<String>]) {
    if lookup(doc.as_table_mut(), &added.keys, added.index).is_none_or(|t| t.position().is_some()) {
        return;
    }
    let previous = added.index.and_then(|i| i.checked_sub(1));
    let after_previous = previous
        .and_then(|i| lookup(doc.as_table_mut(), &added.keys, Some(i)))
        .and_then(|t| t.position())
        .map(|p| p + 1);
    let successor = || {
        order.iter().skip_while(|p| **p != added.keys).skip(1).find_map(|p| {
            let pos = lookup_ref(doc.as_table(), p)?.position()?;
            Some((p.clone(), pos))
        })
    };
    let (at, before) = match after_previous {
        Some(at) => (at, None),
        None => match successor() {
            Some((path, at)) => (at, Some(path)),
            None => (max_position(doc.as_table()) + 1, None),
        },
    };
    // Comments above the first header are stored on that table. A table
    // placed above it takes the file header along, or a `#:schema` line
    // would end up below the new header.
    let header = match &before {
        Some(path) if at == min_position(doc.as_table()) && !doc.iter().any(|(_, i)| i.is_value()) => {
            lookup(doc.as_table_mut(), path, Some(0)).map(|first| {
                let decor = first.decor_mut();
                let prefix = decor.prefix().and_then(|r| r.as_str()).unwrap_or("").to_owned();
                let (moved, kept) = prefix.split_at(file_header(&prefix).len());
                let kept = if kept.starts_with('\n') {
                    kept.to_owned()
                } else {
                    format!("\n{kept}")
                };
                decor.set_prefix(kept);
                moved.to_owned()
            })
        }
        _ => None,
    };
    shift_positions(doc.as_table_mut(), at);
    if let Some(table) = lookup(doc.as_table_mut(), &added.keys, added.index) {
        table.set_position(Some(at));
        if let Some(header) = header {
            set_header(table, header);
        }
    }
}

/// Set `header` on the table whose header prints first: `table`, or for a
/// header-less one, its first sub-table.
fn set_header(table: &mut Table, header: String) {
    if table.is_implicit()
        && let Some((_, Item::Table(child))) = table.iter_mut().find(|(_, i)| i.is_table())
    {
        return set_header(child, header);
    }
    table.decor_mut().set_prefix(header);
}

/// The part of the first table's leading comment that belongs to the file:
/// everything up to its last blank line, else its `#:` directive lines.
fn file_header(prefix: &str) -> &str {
    if let Some(section) = section_comment(prefix) {
        return section;
    }
    let directives: usize = prefix
        .split_inclusive('\n')
        .take_while(|line| line.starts_with("#:"))
        .map(str::len)
        .sum();
    &prefix[..directives]
}

fn min_position(table: &Table) -> isize {
    let mut min = table.position().unwrap_or(isize::MAX);
    for (_, item) in table.iter() {
        let child = match item {
            Item::Table(t) => min_position(t),
            Item::ArrayOfTables(a) => a.iter().map(min_position).min().unwrap_or(isize::MAX),
            _ => isize::MAX,
        };
        min = min.min(child);
    }
    min
}

fn lookup<'a>(root: &'a mut Table, keys: &[String], index: Option<usize>) -> Option<&'a mut Table> {
    let (last, parents) = keys.split_last()?;
    let mut table = root;
    for key in parents {
        table = match table.get_mut(key)? {
            Item::Table(t) => t,
            Item::ArrayOfTables(a) => a.get_mut(0)?,
            _ => return None,
        };
    }
    match table.get_mut(last)? {
        Item::Table(t) => Some(t),
        Item::ArrayOfTables(a) => a.get_mut(index.unwrap_or(0)),
        _ => None,
    }
}

/// The first table at `keys` (an array's first element), read-only.
fn lookup_ref<'a>(root: &'a Table, keys: &[String]) -> Option<&'a Table> {
    let mut table = root;
    for key in keys {
        table = match table.get(key)? {
            Item::Table(t) => t,
            Item::ArrayOfTables(a) => a.get(0)?,
            _ => return None,
        };
    }
    Some(table)
}

fn shift_positions(table: &mut Table, at: isize) {
    if let Some(pos) = table.position()
        && pos >= at
    {
        table.set_position(Some(pos + 1));
    }
    for (_, item) in table.iter_mut() {
        match item {
            Item::Table(t) => shift_positions(t, at),
            Item::ArrayOfTables(a) => a.iter_mut().for_each(|t| shift_positions(t, at)),
            _ => {}
        }
    }
}

fn max_position(table: &Table) -> isize {
    let mut max = table.position().unwrap_or(0);
    for (_, item) in table.iter() {
        let child = match item {
            Item::Table(t) => max_position(t),
            Item::ArrayOfTables(a) => a.iter().map(max_position).max().unwrap_or(0),
            _ => 0,
        };
        max = max.max(child);
    }
    max
}

#[cfg(test)]
mod tests {
    use super::*;

    fn merged(existing: &str, desired: &str) -> String {
        merge(existing, desired).expect("valid TOML")
    }

    const USER: &str = "\
#:schema https://grimoire.rs/schemas/grimoire.schema.json
# Team config — keep the reviewers first.

[options]
clients = [\"claude\"] # only claude for now

[skills]
# reviewers
zeta = 'ghcr.io/acme/skills/zeta:1'
alpha = \"ghcr.io/acme/skills/alpha\"

# helpers
helper = \"ghcr.io/acme/skills/helper:2\"

[rules]
";

    const RENDERED: &str = "\
[options]
clients = [\"claude\"]

[skills]
alpha = \"ghcr.io/acme/skills/alpha:latest\"
helper = \"ghcr.io/acme/skills/helper:2\"
zeta = \"ghcr.io/acme/skills/zeta:1\"

[rules]
";

    #[test]
    fn unchanged_model_leaves_every_byte() {
        assert_eq!(merged(USER, RENDERED), USER);
    }

    #[test]
    fn added_skill_is_appended_and_nothing_else_moves() {
        let desired = RENDERED.replace(
            "helper = \"ghcr.io/acme/skills/helper:2\"\n",
            "beta = \"ghcr.io/acme/skills/beta:1\"\nhelper = \"ghcr.io/acme/skills/helper:2\"\n",
        );
        let expected = USER.replace(
            "helper = \"ghcr.io/acme/skills/helper:2\"\n",
            "helper = \"ghcr.io/acme/skills/helper:2\"\nbeta = \"ghcr.io/acme/skills/beta:1\"\n",
        );
        assert_eq!(merged(USER, &desired), expected);
    }

    #[test]
    fn changed_value_keeps_position_and_trailing_comment() {
        let desired = RENDERED.replace("clients = [\"claude\"]", "clients = [\"claude\", \"codex\"]");
        let expected = USER.replace(
            "clients = [\"claude\"] # only claude for now",
            "clients = [\"claude\", \"codex\"] # only claude for now",
        );
        assert_eq!(merged(USER, &desired), expected);
    }

    #[test]
    fn removed_entry_takes_its_own_comment() {
        let desired = RENDERED.replace("zeta = \"ghcr.io/acme/skills/zeta:1\"\n", "");
        let expected = USER.replace("# reviewers\nzeta = 'ghcr.io/acme/skills/zeta:1'\n", "");
        assert_eq!(merged(USER, &desired), expected);
    }

    #[test]
    fn removed_entry_hands_a_section_comment_to_the_next_key() {
        let existing = "[skills]\na = \"ghcr.io/acme/skills/a:1\"\n\n# helpers\n\n# the old one\nold = \"ghcr.io/acme/skills/old:1\"\nnew = \"ghcr.io/acme/skills/new:1\"\n\n[rules]\n";
        let desired = "[skills]\na = \"ghcr.io/acme/skills/a:1\"\nnew = \"ghcr.io/acme/skills/new:1\"\n\n[rules]\n";
        assert_eq!(
            merged(existing, desired),
            "[skills]\na = \"ghcr.io/acme/skills/a:1\"\n\n# helpers\n\nnew = \"ghcr.io/acme/skills/new:1\"\n\n[rules]\n"
        );
    }

    #[test]
    fn removing_the_last_entry_keeps_the_table() {
        let desired = "[skills]\n\n[rules]\n";
        let out = merged(
            "[skills]\n# only one\na = \"ghcr.io/acme/skills/a:1\"\n\n[rules]\n",
            desired,
        );
        assert_eq!(out, "[skills]\n\n[rules]\n");
    }

    #[test]
    fn new_table_lands_at_its_canonical_slot() {
        let desired = RENDERED.replace(
            "[skills]",
            "[options.tui]\ndefault_view = \"tree\"\n\n[[registries]]\nalias = \"acme\"\noci = \"ghcr.io/acme\"\n\n[skills]",
        ) + "\n[agents]\nreviewer = \"ghcr.io/acme/agents/reviewer:1\"\n";
        let out = merged(USER, &desired);
        let tui = out.find("[options.tui]").expect("tui");
        let registries = out.find("[[registries]]").expect("registries");
        let skills = out.find("[skills]").expect("skills");
        let rules = out.find("[rules]").expect("rules");
        let agents = out.find("[agents]").expect("agents");
        assert!(tui < registries && registries < skills && rules < agents, "{out}");
        assert!(out.starts_with("#:schema"), "{out}");
    }

    #[test]
    fn new_options_table_over_only_a_sub_table_stays_headerless() {
        let out = merged(
            "[skills]\n\n[rules]\n",
            "[options]\n\n[options.tui]\nsort = \"name\"\n\n[skills]\n\n[rules]\n",
        );
        assert_eq!(out, "[options.tui]\nsort = \"name\"\n\n[skills]\n\n[rules]\n");
    }

    #[test]
    fn new_first_table_goes_below_the_file_header() {
        let existing =
            "#:schema https://x.example/s.json\n# header\n\n[skills]\na = \"ghcr.io/acme/skills/a:1\"\n\n[rules]\n";
        let desired = "[options]\nclients = [\"claude\"]\n\n[skills]\na = \"ghcr.io/acme/skills/a:1\"\n\n[rules]\n";
        assert_eq!(
            merged(existing, desired),
            "#:schema https://x.example/s.json\n# header\n\n[options]\nclients = [\"claude\"]\n\n[skills]\na = \"ghcr.io/acme/skills/a:1\"\n\n[rules]\n"
        );
        let tight = "#:schema https://x.example/s.json\n[skills]\n";
        let out = merged(tight, "[options]\nclients = [\"claude\"]\n\n[skills]\n");
        assert!(
            out.starts_with("#:schema https://x.example/s.json\n[options]\n"),
            "{out}"
        );
    }

    #[test]
    fn missing_empty_tables_are_not_added() {
        let out = merged("[skills]\na = \"ghcr.io/acme/skills/a:1\"\n", RENDERED_EMPTY_RULES);
        assert_eq!(out, "[skills]\na = \"ghcr.io/acme/skills/a:1\"\n");
    }

    const RENDERED_EMPTY_RULES: &str = "[skills]\na = \"ghcr.io/acme/skills/a:1\"\n\n[rules]\n";

    #[test]
    fn emptied_table_is_dropped_unless_its_header_is_commented() {
        let desired = "[skills]\n\n[rules]\n";
        let plain = merged(
            "[skills]\n\n[rules]\n\n[agents]\nr = \"ghcr.io/acme/agents/r:1\"\n",
            desired,
        );
        assert!(!plain.contains("[agents]"), "{plain}");
        let commented = merged(
            "[skills]\n\n[rules]\n\n# agents go here\n[agents]\nr = \"ghcr.io/acme/agents/r:1\"\n",
            desired,
        );
        assert!(commented.contains("# agents go here\n[agents]\n"), "{commented}");
        assert!(!commented.contains("r = "), "{commented}");
    }

    #[test]
    fn default_valued_keys_survive_an_unrelated_write() {
        let existing = "[options]\nshow_deprecated = false\nclients = []\n\n[skills]\n\n[rules]\n";
        assert_eq!(merged(existing, "[skills]\n\n[rules]\n"), existing);
        let unset = merged(
            "[options]\nshow_deprecated = true\n\n[skills]\n\n[rules]\n",
            "[skills]\n\n[rules]\n",
        );
        assert!(!unset.contains("show_deprecated"), "{unset}");
    }

    #[test]
    fn inline_and_dotted_declaration_tables_edit_in_place() {
        let desired = "[skills]\na = \"ghcr.io/acme/skills/a:1\"\nb = \"ghcr.io/acme/skills/b:1\"\n\n[rules]\n";
        let inline = merged("skills = { a = \"ghcr.io/acme/skills/a:1\" }\n", desired);
        assert_eq!(
            inline,
            "skills = { a = \"ghcr.io/acme/skills/a:1\", b = \"ghcr.io/acme/skills/b:1\" }\n"
        );
        let dotted = merged("skills.a = \"ghcr.io/acme/skills/a:1\"\n", desired);
        assert_eq!(
            dotted,
            "skills.a = \"ghcr.io/acme/skills/a:1\"\nskills.b = \"ghcr.io/acme/skills/b:1\"\n"
        );
    }

    const REGISTRIES: &str = "\
# first
[[registries]]
alias = \"one\"
oci = \"ghcr.io/one\"

# second, the default
[[registries]]
alias = \"two\"
oci = \"ghcr.io/two\"
default = true

# third
[[registries]]
alias = \"three\"
index = \"https://index.example\"

[skills]

[rules]
";

    #[test]
    fn registry_edit_keeps_position_and_comments() {
        let desired = "\
[[registries]]
alias = \"one\"
oci = \"ghcr.io/one\"
default = true

[[registries]]
alias = \"two\"
oci = \"ghcr.io/two-moved\"

[[registries]]
alias = \"three\"
index = \"https://index.example\"

[skills]

[rules]
";
        let expected = REGISTRIES
            .replace("oci = \"ghcr.io/one\"\n", "oci = \"ghcr.io/one\"\ndefault = true\n")
            .replace(
                "oci = \"ghcr.io/two\"\ndefault = true\n",
                "oci = \"ghcr.io/two-moved\"\n",
            );
        assert_eq!(merged(REGISTRIES, desired), expected);
    }

    #[test]
    fn registry_removal_drops_exactly_that_entry() {
        let desired = "\
[[registries]]
alias = \"one\"
oci = \"ghcr.io/one\"

[[registries]]
alias = \"three\"
index = \"https://index.example\"

[skills]

[rules]
";
        let expected = REGISTRIES.replace(
            "# second, the default\n[[registries]]\nalias = \"two\"\noci = \"ghcr.io/two\"\ndefault = true\n\n",
            "",
        );
        assert_eq!(merged(REGISTRIES, desired), expected);
    }

    #[test]
    fn added_registry_follows_the_last_one() {
        let desired = REGISTRIES.replace(
            "[skills]",
            "[[registries]]\nalias = \"four\"\noci = \"ghcr.io/four\"\n\n[skills]",
        );
        let desired = desired
            .replace("# first\n", "")
            .replace("# second, the default\n", "")
            .replace("# third\n", "");
        let out = merged(REGISTRIES, &desired);
        let three = out.find("alias = \"three\"").expect("three");
        let four = out.find("alias = \"four\"").expect("four");
        let skills = out.find("[skills]").expect("skills");
        assert!(three < four && four < skills, "{out}");
        assert!(out.contains("# second, the default\n"), "{out}");
    }

    #[test]
    fn declaration_short_form_is_not_rewritten() {
        let existing = "[skills]\na = \"ghcr.io/acme/skills/a\"\n\n[rules]\n";
        assert_eq!(
            merged(existing, "[skills]\na = \"ghcr.io/acme/skills/a:latest\"\n\n[rules]\n"),
            existing
        );
    }
}
