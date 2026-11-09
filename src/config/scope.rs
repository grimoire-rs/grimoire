// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! Which configuration scope a command operates on.

use std::borrow::Cow;
use std::path::Path;

use crate::oci::text::{MAX_REGISTRY_TEXT, escape_controls};

/// The two independent configuration scopes.
///
/// Global (`$GRIM_HOME/grimoire.toml`) and project (discovered by walking
/// up from the working directory) each own their own lock and are **never
/// merged** — a command operates on exactly one scope.
///
/// Closed internal enum: the binary is the only consumer, so matches stay
/// total — no `#[non_exhaustive]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ConfigScope {
    /// The per-user global declaration under `$GRIM_HOME`.
    Global,
    /// A project-local declaration discovered by walking up from the CWD.
    Project,
}

/// The `grim` a printed remedy names, so the command acts on the scope the run
/// resolved. A bare `grim` acts on whatever project the shell's directory walks
/// up to, so a global or `--config` run that printed one would have its remedy
/// move another project's pin or delete its files.
///
/// Built once, where the scope is resolved
/// ([`crate::command::scope_resolution::ResolvedScope::invocation`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invocation(Cow<'static, str>);

impl Invocation {
    /// `grim --global`.
    pub const GLOBAL: Self = Self(Cow::Borrowed("grim --global"));

    /// Whether this is [`Self::GLOBAL`], whose files live under `$GRIM_HOME`
    /// rather than in a project.
    pub fn is_global(&self) -> bool {
        *self == Self::GLOBAL
    }

    /// Plain `grim`, for a project found by walking up from the working
    /// directory: the user's shell already stands where it finds that project.
    pub const WALKED: Self = Self(Cow::Borrowed("grim"));

    /// `grim --config <path>`, for a config named explicitly or found from
    /// anywhere but the working directory. The path is made absolute here: a
    /// relative one names another file once the user runs the remedy from
    /// elsewhere. With no working directory to read it stays as given.
    pub fn config(path: &Path) -> Self {
        let absolute = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
        Self(Cow::Owned(format!(
            "grim --config {}",
            shell_word(&absolute.to_string_lossy())
        )))
    }

    /// The named update that moves `name`'s pin, the one form every note prints.
    /// Without the `--` a name with a leading dash reads as a flag. A name reaches
    /// here from a lock, so it stays one shell word, or `<name>` where none fits.
    pub fn update(&self, name: &str) -> String {
        self.update_for(name, ShellFamily::HOST)
    }

    fn update_for(&self, name: &str, family: ShellFamily) -> String {
        let name = name_word(name, family).unwrap_or_else(|| "<name>".to_string());
        format!("{self} update -- {name}")
    }

    /// The `uninstall` of one artifact, with `flags` (each ending in a space)
    /// before its kind, or the `<kind> <name>` form where no shell word names it.
    pub fn uninstall(&self, flags: &str, kind: impl std::fmt::Display, name: &str) -> String {
        self.uninstall_for(flags, kind, name, ShellFamily::HOST)
    }

    fn uninstall_for(&self, flags: &str, kind: impl std::fmt::Display, name: &str, family: ShellFamily) -> String {
        match unflagged_word(name, family) {
            Some(word) => format!("{self} uninstall {flags}{kind} {word}"),
            None => format!("{self} uninstall {flags}<kind> <name>"),
        }
    }

    /// The `remove` of one binding, or the `<kind> <name>` form where no shell
    /// word names it.
    pub fn remove(&self, kind: impl std::fmt::Display, name: &str) -> String {
        match unflagged_word(name, ShellFamily::HOST) {
            Some(word) => format!("{self} remove {kind} {word}"),
            None => format!("{self} remove <kind> <name>"),
        }
    }
}

impl std::fmt::Display for Invocation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// The shells a printed command is quoted for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ShellFamily {
    Posix,
    /// PowerShell, the Windows default, and cmd.exe.
    Windows,
}

impl ShellFamily {
    /// The family of the shell a user of this build pastes a command into.
    const HOST: Self = if cfg!(windows) { Self::Windows } else { Self::Posix };
}

/// `text`, a path, as one shell word for the host's shells, or `<path>` where
/// no form reads back as one inert word in all of them. Hidden code points are
/// spelled out first, so a path cannot drive the terminal; a path holding one
/// no longer copies exactly, which is the price of keeping escape sequences off
/// the screen.
pub fn shell_word(text: &str) -> String {
    shell_word_for(text, ShellFamily::HOST)
}

fn shell_word_for(text: &str, family: ShellFamily) -> String {
    word_for(text, family).unwrap_or_else(|| "<path>".to_string())
}

/// `text` as one shell word for `family`, its hidden code points spelled out,
/// or `None` where no form is one inert word there.
fn word_for(text: &str, family: ShellFamily) -> Option<String> {
    quote_for(escape_controls(text), family)
}

fn quote_for(shown: String, family: ShellFamily) -> Option<String> {
    match family {
        ShellFamily::Posix => Some(posix_word(shown)),
        ShellFamily::Windows => windows_word(shown),
    }
}

/// `name`, a lock row's name, as one shell word after a `--`, or `None` where
/// no word names that row alone.
fn name_word(name: &str, family: ShellFamily) -> Option<String> {
    let shown = escape_controls(name);
    // A longer name reaches a terminal cut, and a hidden code point spelled out:
    // either way the printed word may name another row.
    if name.chars().count() > MAX_REGISTRY_TEXT || shown != name {
        return None;
    }
    // PowerShell may drop the `--` before the name, which then reads as a flag.
    if family == ShellFamily::Windows && name.starts_with('-') {
        return None;
    }
    quote_for(shown, family)
}

/// [`name_word`] for a command that prints no `--`, where a leading dash reads
/// as a flag in every shell.
fn unflagged_word(name: &str, family: ShellFamily) -> Option<String> {
    if name.starts_with('-') {
        return None;
    }
    name_word(name, family)
}

/// `shown` bare when every character is safe unquoted in both shells, else
/// double-quoted, which both read as one word. PowerShell expands `$` and a
/// backtick inside double quotes, and a native program reads a closing `\"` as
/// an escaped quote, so such text takes PowerShell's literal single quotes
/// instead, each quote character doubled. `None` for text no form carries.
fn windows_word(shown: String) -> Option<String> {
    let safe = |c: char| c.is_ascii_alphanumeric() || matches!(c, '\\' | '/' | '.' | '_' | '-' | ':');
    if !shown.is_empty() && shown.chars().all(safe) {
        return Some(shown);
    }
    // cmd.exe keeps single quotes as text, so an operator inside them runs, and
    // it expands `%` and `!` inside double quotes; Windows PowerShell 5.1 splits
    // an argument holding `"` into several.
    if shown
        .chars()
        .any(|c| matches!(c, '"' | '&' | '|' | '<' | '>' | '^' | '(' | ')' | '%' | '!'))
    {
        return None;
    }
    let expands = |c: char| matches!(c, '$' | '`' | '\u{201c}' | '\u{201d}' | '\u{201e}');
    if !shown.chars().any(expands) && !shown.ends_with('\\') {
        return Some(format!("\"{shown}\""));
    }
    let mut quoted = String::from("'");
    for c in shown.chars() {
        // PowerShell closes a single-quoted string on any of these.
        if matches!(c, '\'' | '\u{2018}' | '\u{2019}' | '\u{201a}' | '\u{201b}') {
            quoted.push(c);
        }
        quoted.push(c);
    }
    quoted.push('\'');
    Some(quoted)
}

/// `shown` unchanged when every character is safe unquoted, otherwise
/// single-quoted with each `'` closed, escaped and reopened. A `\` goes outside
/// the quotes the same way: fish reads `\'` and `\\` inside them as escapes, so
/// a raw one moves where the word ends and can run the rest as a command.
fn posix_word(shown: String) -> String {
    let safe = |c: char| c.is_ascii_alphanumeric() || matches!(c, '/' | '.' | '_' | '-' | ':' | '@' | '+' | ',' | '%');
    if !shown.is_empty() && shown.chars().all(safe) {
        return shown;
    }
    let mut quoted = String::from("'");
    for c in shown.chars() {
        match c {
            '\'' => quoted.push_str(r"'\''"),
            '\\' => quoted.push_str(r"'\\'"),
            c => quoted.push(c),
        }
    }
    quoted.push('\'');
    quoted
}

impl std::fmt::Display for ConfigScope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Global => "global",
            Self::Project => "project",
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_is_lowercase() {
        assert_eq!(ConfigScope::Global.to_string(), "global");
        assert_eq!(ConfigScope::Project.to_string(), "project");
    }

    #[test]
    fn a_global_remedy_names_the_global_flag() {
        assert_eq!(Invocation::GLOBAL.to_string(), "grim --global");
        assert_eq!(Invocation::WALKED.to_string(), "grim");
    }

    #[test]
    fn a_config_remedy_names_its_path() {
        let plain = Invocation::config(Path::new("/srv/team-a/grimoire.toml"));
        assert_eq!(plain.to_string(), "grim --config /srv/team-a/grimoire.toml");
        assert_eq!(plain.to_string(), "grim --config /srv/team-a/grimoire.toml");
    }

    /// A shell splits the printed command back into the path the run used.
    #[test]
    fn a_config_path_outside_the_safe_set_is_one_quoted_word() {
        for (path, shown) in [
            ("/my projects/grimoire.toml", "'/my projects/grimoire.toml'"),
            ("/a/it's/grimoire.toml", r"'/a/it'\''s/grimoire.toml'"),
            ("/a/$HOME/`x`;rm/grimoire.toml", "'/a/$HOME/`x`;rm/grimoire.toml'"),
            ("/a/*?[b]/grimoire.toml", "'/a/*?[b]/grimoire.toml'"),
        ] {
            assert_eq!(
                Invocation::config(Path::new(path)).to_string(),
                format!("grim --config {shown}"),
                "{path}"
            );
        }
    }

    /// The path is user-controlled text on its way to a terminal.
    #[test]
    fn a_control_character_in_a_config_path_never_reaches_the_terminal() {
        let shown = Invocation::config(Path::new("/a/\u{1b}[2J\n/grimoire.toml")).to_string();
        assert!(!shown.contains('\u{1b}') && !shown.contains('\n'), "{shown:?}");
        assert_eq!(shown, r"grim --config '/a/'\\'u{1b}[2J'\\'n/grimoire.toml'");
    }

    /// Run from elsewhere, a relative path names another file or none at all.
    #[test]
    fn a_relative_config_path_renders_absolute() {
        let cwd = std::env::current_dir().unwrap();
        let expected = Invocation::config(&cwd.join("other/grimoire.toml"));

        assert_eq!(Invocation::config(Path::new("other/grimoire.toml")), expected);
        assert_eq!(Invocation::config(Path::new("./other/grimoire.toml")), expected);
    }

    #[test]
    fn every_scope_prints_one_form_of_a_named_update() {
        assert_eq!(Invocation::WALKED.update("x"), "grim update -- x");
        assert_eq!(Invocation::GLOBAL.update("x"), "grim --global update -- x");
        let config = Invocation::config(Path::new("/srv/my team/grimoire.toml"));
        assert_eq!(
            config.update("x"),
            "grim --config '/srv/my team/grimoire.toml' update -- x"
        );
    }

    /// What a POSIX shell is handed stays byte for byte what it was, for text
    /// with no backslash.
    #[test]
    fn posix_quoting_is_unchanged() {
        for (text, shown) in [
            ("/srv/team-a/grimoire.toml", "/srv/team-a/grimoire.toml"),
            ("a@b+c,d%e:f", "a@b+c,d%e:f"),
            ("/my projects/grimoire.toml", "'/my projects/grimoire.toml'"),
            ("/a/it's/grimoire.toml", r"'/a/it'\''s/grimoire.toml'"),
            ("", "''"),
        ] {
            assert_eq!(shell_word_for(text, ShellFamily::Posix), shown, "{text}");
        }
    }

    /// PowerShell, the default, reads each word as the path, and cmd.exe a bare or
    /// double-quoted one. Single quotes are kept for text double quotes cannot carry:
    /// PowerShell expands `$` and a backtick inside them, and a native program reads
    /// `\"` as an escaped quote. cmd.exe reads single quotes as part of the path.
    #[test]
    fn a_windows_path_is_one_word_for_powershell_and_cmd() {
        for (text, shown) in [
            (r"C:\Users\me\grimoire.toml", r"C:\Users\me\grimoire.toml"),
            (r"C:\Users\my team\grimoire.toml", r#""C:\Users\my team\grimoire.toml""#),
            (r"C:\o'neil\grimoire.toml", r#""C:\o'neil\grimoire.toml""#),
            (r"C:\a$b\grimoire.toml", r"'C:\a$b\grimoire.toml'"),
            (r"C:\a`b\grimoire.toml", r"'C:\a`b\grimoire.toml'"),
            (r"C:\it's $x\grimoire.toml", r"'C:\it''s $x\grimoire.toml'"),
            ("C:\\it\u{2019}s $x", "'C:\\it\u{2019}\u{2019}s $x'"),
            (r"C:\my dir\", r"'C:\my dir\'"),
            ("C:\\a\u{1b}b", r#""C:\a\u{1b}b""#),
            ("", r#""""#),
        ] {
            assert_eq!(shell_word_for(text, ShellFamily::Windows), shown, "{text}");
        }
    }

    #[test]
    fn a_printed_word_is_quoted_for_the_host() {
        assert_eq!(ShellFamily::HOST == ShellFamily::Windows, cfg!(windows));
        assert_eq!(shell_word("a b"), shell_word_for("a b", ShellFamily::HOST));
    }

    /// A lock's row name is free text a shell would otherwise run.
    #[test]
    fn a_name_in_a_printed_command_is_one_shell_word() {
        assert_eq!(Invocation::WALKED.update("x; y"), "grim update -- 'x; y'");
        assert_eq!(Invocation::WALKED.update("a$b"), "grim update -- 'a$b'");
    }

    /// fish reads `\\` and `\'` as escapes inside single quotes, so a backslash is
    /// written outside them, where fish and POSIX shells read it alike.
    #[test]
    fn a_backslash_is_written_outside_the_single_quotes() {
        for (text, shown) in [
            (r"x\'; y", r"'x'\\''\''; y'"),
            (r"C:\Users\me\grimoire.toml", r"'C:'\\'Users'\\'me'\\'grimoire.toml'"),
            (r"a\\b", r"'a'\\''\\'b'"),
            (r"end\", r"'end'\\''"),
        ] {
            assert_eq!(shell_word_for(text, ShellFamily::Posix), shown, "{text}");
        }
    }

    /// cmd.exe keeps single quotes as text and expands `%` and `!` inside double
    /// quotes, and Windows PowerShell 5.1 splits an argument at an embedded `"`, so
    /// no form is one inert word in both and the remedy names a placeholder.
    #[test]
    fn a_windows_word_no_form_carries_gets_the_generic_form() {
        for text in [
            r"C:\Users\me\100% & x\grimoire.toml",
            r"C:\Users\me\$x & y\grimoire.toml",
            r"C:\a;b&c(d)\grimoire.toml",
            r"C:\100%\grimoire.toml",
            r"C:\a|b",
            r"C:\a<b",
            r"C:\a>b",
            r"C:\a^b",
            r"C:\a(b",
            r"C:\a)b",
            r"C:\a!b!",
            r#"x" "y"#,
            r#"C:\a b\x" "y"#,
        ] {
            assert_eq!(shell_word_for(text, ShellFamily::Windows), "<path>", "{text}");
            assert_eq!(
                Invocation::WALKED.uninstall_for("", "mcp", text, ShellFamily::Windows),
                "grim uninstall <kind> <name>",
                "{text}"
            );
            assert_eq!(
                Invocation::WALKED.update_for(text, ShellFamily::Windows),
                "grim update -- <name>",
                "{text}"
            );
        }
    }

    /// PowerShell may drop the `--` before a native command's argument, so on
    /// Windows a name with a leading dash would read as a flag.
    #[test]
    fn a_leading_dash_name_gets_no_windows_command() {
        assert_eq!(
            Invocation::WALKED.update_for("-x", ShellFamily::Windows),
            "grim update -- <name>"
        );
        assert_eq!(
            Invocation::WALKED.update_for("-x", ShellFamily::Posix),
            "grim update -- -x"
        );
        assert_eq!(
            Invocation::WALKED.update_for("x-", ShellFamily::Windows),
            "grim update -- x-"
        );
    }

    /// `uninstall` and `remove` print no `--`, so a leading dash reads as a flag in
    /// every shell.
    #[test]
    fn a_leading_dash_name_gets_no_uninstall_or_remove_command() {
        assert_eq!(
            Invocation::WALKED.uninstall("", "mcp", "-x"),
            "grim uninstall <kind> <name>"
        );
        assert_eq!(Invocation::WALKED.remove("bundle", "-x"), "grim remove <kind> <name>");
        assert_eq!(Invocation::WALKED.remove("bundle", "pack"), "grim remove bundle pack");
        assert_eq!(
            Invocation::WALKED.uninstall("--force ", "mcp", "x y"),
            "grim uninstall --force mcp 'x y'"
        );
    }

    /// A name is cut to [`MAX_REGISTRY_TEXT`] characters before it reaches a
    /// terminal, and the cut may be another row's whole name.
    #[test]
    fn a_name_past_the_display_cap_gets_the_generic_form() {
        let long = "a".repeat(crate::oci::text::MAX_REGISTRY_TEXT + 1);
        let fits = "a".repeat(crate::oci::text::MAX_REGISTRY_TEXT);
        assert_eq!(Invocation::WALKED.update(&long), "grim update -- <name>");
        assert_eq!(
            Invocation::WALKED.uninstall("", "mcp", &long),
            "grim uninstall <kind> <name>"
        );
        assert_eq!(Invocation::WALKED.remove("bundle", &long), "grim remove <kind> <name>");
        assert_eq!(Invocation::WALKED.update(&fits), format!("grim update -- {fits}"));
    }

    /// A hidden code point reaches a terminal spelled out, and the spelling may
    /// be another row's whole name.
    #[test]
    fn a_name_holding_a_hidden_code_point_gets_the_generic_form() {
        assert_eq!(Invocation::WALKED.update("pa\u{1b}[2Jck"), "grim update -- <name>");
        assert_eq!(
            Invocation::WALKED.uninstall("", "skill", "d\u{1b}[2J"),
            "grim uninstall <kind> <name>"
        );
        assert_eq!(
            Invocation::WALKED.remove("bundle", "b\u{7}"),
            "grim remove <kind> <name>"
        );
    }
}
