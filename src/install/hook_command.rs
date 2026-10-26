// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! The command a client runs to reach the native launcher (C-N13).
//!
//! Exec form where the client has one, shell form where it does not. The three
//! operands (`<client> <event> <root>`) are never quoted: their grammar
//! excludes every shell metacharacter. Only the launcher path, which comes from
//! `$GRIM_HOME`, can need quoting, and each shell has its own rule for it.
//!
//! The registration is byte-stable once written: Codex hashes the raw command
//! text for its trust record, so a changed byte un-trusts an approved hook.

use std::path::Path;

use grimoire_hook_core::argv;

use crate::oci::hook::{HookCommand, HookShell};

/// The host a registration is rendered for. A parameter rather than `cfg!` so
/// the Windows renderings are golden-tested on Linux.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostOs {
    Unix,
    Windows,
}

impl HostOs {
    pub const fn current() -> Self {
        Self::from_windows(cfg!(windows))
    }

    pub const fn from_windows(windows: bool) -> Self {
        if windows { Self::Windows } else { Self::Unix }
    }
}

/// The shell a client runs a shell-form command through on Windows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowsShell {
    /// Codex: `cmd /C`, which cannot carry an embedded quote (openai/codex#38168).
    Cmd,
    /// Claude and Qoder: PowerShell, or Git Bash for a bare path.
    PowerShell,
}

/// How a client takes its command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandForm {
    /// `exec` plus `args`, no shell (Copilot).
    Exec,
    /// One command string handed to a shell.
    Shell(WindowsShell),
}

/// The launcher path cannot be written into this client's registration
/// unambiguously: a control character, bytes that are not UTF-8, or a
/// character the Windows `cmd /C` shell cannot quote.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LauncherPathUnquotable;

/// A rendered registration command and the shell it needs, when not the default.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rendered {
    pub command: HookCommand,
    pub shell: Option<HookShell>,
}

/// Render the command that runs `launcher` for `(client, event, root)`.
///
/// # Errors
///
/// [`LauncherPathUnquotable`] for a path this form cannot carry.
pub fn render(
    form: CommandForm,
    launcher: &Path,
    client: &str,
    event: &str,
    root: &str,
    host_os: HostOs,
) -> Result<Rendered, LauncherPathUnquotable> {
    let path = path_text(launcher, host_os)?;
    let [client, event, root] = argv::render(client, event, root);
    match form {
        CommandForm::Exec => Ok(Rendered {
            command: HookCommand::Argv(vec![path, client, event, root]),
            shell: None,
        }),
        CommandForm::Shell(windows) => {
            let (launcher, shell) = shell_launcher(&path, host_os, windows)?;
            Ok(Rendered {
                command: HookCommand::Shell(format!("{launcher} {client} {event} {root}")),
                shell,
            })
        }
    }
}

/// Whether `launcher` can be registered for `form` at all. Lets the registrar
/// report `launcher-path-unquotable` before any hook is assembled.
///
/// # Errors
///
/// [`LauncherPathUnquotable`], exactly when [`render`] would refuse.
pub fn check(form: CommandForm, launcher: &Path, host_os: HostOs) -> Result<(), LauncherPathUnquotable> {
    let path = path_text(launcher, host_os)?;
    match form {
        CommandForm::Exec => Ok(()),
        CommandForm::Shell(windows) => shell_launcher(&path, host_os, windows).map(|_| ()),
    }
}

/// The path as UTF-8 text, without control characters, and never `\\?\`
/// verbatim on Windows (a client may hand the string to a shell that does not
/// understand the prefix).
fn path_text(launcher: &Path, host_os: HostOs) -> Result<String, LauncherPathUnquotable> {
    let text = launcher.to_str().ok_or(LauncherPathUnquotable)?;
    if text.chars().any(char::is_control) {
        return Err(LauncherPathUnquotable);
    }
    Ok(match host_os {
        HostOs::Unix => text.to_owned(),
        HostOs::Windows => strip_verbatim_drive(text).to_owned(),
    })
}

/// `\\?\C:\x` becomes `C:\x`. `\\?\UNC\…` stays: a shared home is refused
/// elsewhere (C-N14), and a UNC path cannot be simplified safely.
fn strip_verbatim_drive(text: &str) -> &str {
    match text.strip_prefix(r"\\?\") {
        Some(rest) if matches!(rest.as_bytes(), [d, b':', b'\\', ..] if d.is_ascii_alphabetic()) => rest,
        _ => text,
    }
}

fn shell_launcher(
    path: &str,
    host_os: HostOs,
    windows: WindowsShell,
) -> Result<(String, Option<HookShell>), LauncherPathUnquotable> {
    match (host_os, windows) {
        (HostOs::Unix, _) => Ok((bare_or(path, is_posix_bare, posix_single_quote), None)),
        (HostOs::Windows, WindowsShell::Cmd) => {
            if path.chars().all(is_cmd_bare) {
                Ok((path.to_owned(), None))
            } else {
                Err(LauncherPathUnquotable)
            }
        }
        (HostOs::Windows, WindowsShell::PowerShell) => {
            let slashed = path.replace('\\', "/");
            if slashed.chars().all(is_powershell_bare) {
                Ok((slashed, None))
            } else {
                Ok((
                    format!("& {}", powershell_single_quote(path)),
                    Some(HookShell::PowerShell),
                ))
            }
        }
    }
}

fn bare_or(path: &str, is_bare: fn(char) -> bool, quote: fn(&str) -> String) -> String {
    if path.chars().all(is_bare) {
        path.to_owned()
    } else {
        quote(path)
    }
}

fn is_posix_bare(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '/' | '+' | '-')
}

fn is_cmd_bare(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | ':' | '\\' | '+' | '-')
}

fn is_powershell_bare(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | ':' | '/' | '+' | '-')
}

/// Wrap `value` in POSIX single quotes, escaping `'` as `'\''`, the only
/// escape `sh` has inside them.
pub(super) fn posix_single_quote(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('\'');
    for c in value.chars() {
        if c == '\'' {
            out.push_str("'\\''");
        } else {
            out.push(c);
        }
    }
    out.push('\'');
    out
}

/// Wrap `value` in PowerShell single quotes. PowerShell treats the typographic
/// quotes U+2018 to U+201B as quote characters too, so each is doubled like `'`.
fn powershell_single_quote(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('\'');
    for c in value.chars() {
        out.push(c);
        if matches!(c, '\'' | '\u{2018}'..='\u{201B}') {
            out.push(c);
        }
    }
    out.push('\'');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const ROOT: &str = "0123456789abcdef0123456789abcdef";
    const SH: CommandForm = CommandForm::Shell(WindowsShell::PowerShell);
    const CMD: CommandForm = CommandForm::Shell(WindowsShell::Cmd);

    #[test]
    fn posix_quoting_neutralizes_every_shape_wp_p0_executed() {
        // The assignment-site rule (B2): a double-quoted literal still expands,
        // so each of these ran under `dash` in the pre-audit form.
        for (raw, quoted) in [
            ("/home/dev/.grimoire", "'/home/dev/.grimoire'"),
            ("/home/my dev/.grimoire", "'/home/my dev/.grimoire'"),
            ("/home/$(touch pwned)/g", "'/home/$(touch pwned)/g'"),
            ("/home/`touch pwned`/g", "'/home/`touch pwned`/g'"),
            ("/home/${HOME}/g", "'/home/${HOME}/g'"),
            ("/home/a;rm -rf b/g", "'/home/a;rm -rf b/g'"),
            ("/home/back\\slash/g", "'/home/back\\slash/g'"),
            ("/home/o'brien/g", "'/home/o'\\''brien/g'"),
        ] {
            assert_eq!(posix_single_quote(raw), quoted, "{raw}");
        }
    }

    fn shell(form: CommandForm, path: &str, host_os: HostOs) -> (String, Option<HookShell>) {
        let rendered = render(form, Path::new(path), "claude", "PreToolUse", ROOT, host_os).unwrap();
        match rendered.command {
            HookCommand::Shell(command) => (command, rendered.shell),
            HookCommand::Argv(argv) => panic!("expected shell form, got {argv:?}"),
        }
    }

    #[test]
    fn unix_shell_bare_path_is_unquoted() {
        assert_eq!(
            shell(SH, "/home/u/.grimoire/hooks/grim-hook", HostOs::Unix),
            (
                format!("/home/u/.grimoire/hooks/grim-hook claude PreToolUse {ROOT}"),
                None
            )
        );
    }

    #[test]
    fn unix_shell_quotes_every_hostile_shape() {
        for (raw, quoted) in [
            ("/home/my dev/h/grim-hook", "'/home/my dev/h/grim-hook'"),
            ("/home/$(touch pwned)/h", "'/home/$(touch pwned)/h'"),
            ("/home/`touch pwned`/h", "'/home/`touch pwned`/h'"),
            ("/home/${HOME}/h", "'/home/${HOME}/h'"),
            ("/home/a;rm -rf b/h", "'/home/a;rm -rf b/h'"),
            ("/home/back\\slash/h", "'/home/back\\slash/h'"),
            ("/home/o'brien/h", "'/home/o'\\''brien/h'"),
            ("/home/~u/h", "'/home/~u/h'"),
        ] {
            assert_eq!(
                shell(SH, raw, HostOs::Unix).0,
                format!("{quoted} claude PreToolUse {ROOT}"),
                "{raw}"
            );
        }
    }

    #[test]
    fn the_shell_form_is_identical_for_codex_and_claude_on_unix() {
        assert_eq!(
            shell(CMD, "/home/u/g", HostOs::Unix),
            shell(SH, "/home/u/g", HostOs::Unix)
        );
    }

    #[test]
    fn windows_cmd_bare_path_is_native_and_unquoted() {
        assert_eq!(
            shell(CMD, r"C:\Users\dev\.grimoire\hooks\grim-hook.exe", HostOs::Windows),
            (
                format!(r"C:\Users\dev\.grimoire\hooks\grim-hook.exe claude PreToolUse {ROOT}"),
                None
            )
        );
    }

    #[test]
    fn windows_cmd_refuses_what_it_cannot_quote() {
        for path in [
            r"C:\Users\John Doe\h\grim-hook.exe",
            r"C:\Users\o'b\h\g.exe",
            r"C:\a&b\g.exe",
        ] {
            assert_eq!(
                render(CMD, Path::new(path), "codex", "Stop", ROOT, HostOs::Windows),
                Err(LauncherPathUnquotable),
                "{path}"
            );
            assert_eq!(
                check(CMD, Path::new(path), HostOs::Windows),
                Err(LauncherPathUnquotable)
            );
        }
    }

    #[test]
    fn windows_powershell_bare_path_uses_forward_slashes() {
        assert_eq!(
            shell(SH, r"C:\Users\dev\.grimoire\hooks\grim-hook.exe", HostOs::Windows),
            (
                format!("C:/Users/dev/.grimoire/hooks/grim-hook.exe claude PreToolUse {ROOT}"),
                None
            )
        );
    }

    #[test]
    fn windows_powershell_quotes_and_asks_for_the_powershell_shell() {
        assert_eq!(
            shell(SH, r"C:\Users\John Doe\.grimoire\hooks\grim-hook.exe", HostOs::Windows),
            (
                format!(r"& 'C:\Users\John Doe\.grimoire\hooks\grim-hook.exe' claude PreToolUse {ROOT}"),
                Some(HookShell::PowerShell)
            )
        );
        // Every quote PowerShell recognises is doubled, the ASCII one and
        // U+2018 to U+201B.
        assert_eq!(
            shell(SH, "C:\\o'b\u{2018}c\u{201B}d\\g.exe", HostOs::Windows).0,
            format!("& 'C:\\o''b\u{2018}\u{2018}c\u{201B}\u{201B}d\\g.exe' claude PreToolUse {ROOT}")
        );
    }

    #[test]
    fn a_verbatim_drive_prefix_is_stripped_but_unc_is_kept() {
        assert_eq!(
            shell(CMD, r"\\?\C:\Users\dev\g.exe", HostOs::Windows).0,
            format!(r"C:\Users\dev\g.exe claude PreToolUse {ROOT}")
        );
        assert_eq!(
            shell(SH, r"\\?\C:\Users\dev\g.exe", HostOs::Windows).0,
            format!("C:/Users/dev/g.exe claude PreToolUse {ROOT}")
        );
        assert_eq!(strip_verbatim_drive(r"\\?\UNC\srv\share\g"), r"\\?\UNC\srv\share\g");
        // Unix never strips: the prefix is an ordinary (hostile) path there.
        assert_eq!(
            shell(SH, r"/h/\\?\C:\g", HostOs::Unix).0,
            format!(r"'/h/\\?\C:\g' claude PreToolUse {ROOT}")
        );
    }

    #[test]
    fn exec_form_carries_the_path_and_operands_as_an_argv_unquoted() {
        for (path, host_os) in [
            ("/home/my dev/it's/grim-hook", HostOs::Unix),
            (r"C:\Users\John Doe\grim-hook.exe", HostOs::Windows),
        ] {
            let rendered = render(
                CommandForm::Exec,
                Path::new(path),
                "copilot",
                "PreToolUse",
                ROOT,
                host_os,
            )
            .unwrap();
            assert_eq!(
                rendered.command,
                HookCommand::Argv(vec![
                    path.to_owned(),
                    "copilot".to_owned(),
                    "PreToolUse".to_owned(),
                    ROOT.to_owned()
                ])
            );
            assert_eq!(rendered.shell, None);
        }
    }

    #[test]
    fn numeric_looking_roots_render_verbatim() {
        let zeros = "0".repeat(32);
        let rendered = render(SH, Path::new("/h/g"), "claude", "Stop", &zeros, HostOs::Unix).unwrap();
        assert_eq!(
            rendered.command,
            HookCommand::Shell(format!("/h/g claude Stop {zeros}"))
        );
    }

    #[test]
    fn a_control_character_or_non_utf8_path_is_refused_in_every_form() {
        for form in [CommandForm::Exec, SH, CMD] {
            for host_os in [HostOs::Unix, HostOs::Windows] {
                assert_eq!(
                    render(form, Path::new("/h/a\nb/g"), "claude", "Stop", ROOT, host_os),
                    Err(LauncherPathUnquotable)
                );
                assert_eq!(
                    check(form, Path::new("/h/a\tb/g"), host_os),
                    Err(LauncherPathUnquotable)
                );
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_non_utf8_path_is_refused() {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt;
        let path = Path::new(OsStr::from_bytes(b"/h/\xff/g"));
        assert_eq!(
            render(SH, path, "claude", "Stop", ROOT, HostOs::Unix),
            Err(LauncherPathUnquotable)
        );
    }

    #[test]
    fn an_attacker_shaped_operand_free_path_changes_only_the_path_token() {
        // C-018b: the operands never depend on the path.
        let (a, _) = shell(SH, "/h/g", HostOs::Unix);
        let (b, _) = shell(SH, "/h/$(id)/g", HostOs::Unix);
        assert!(a.ends_with(&format!(" claude PreToolUse {ROOT}")));
        assert!(b.ends_with(&format!(" claude PreToolUse {ROOT}")));
    }
}
