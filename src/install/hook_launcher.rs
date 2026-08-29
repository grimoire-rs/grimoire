// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! The generated launcher shim and the registered command string (C-008).
//!
//! Two artifacts, one module, because they are two halves of one contract: the
//! shim is what the registration names, and the registration is the only thing
//! that ever names the shim.
//!
//! # Why a shim exists at all
//!
//! The registered string must be **byte-stable** across grim upgrades — Codex
//! hashes the raw command text for its trust record, so a changed string
//! silently un-trusts the hook (`research_hooks_codex_surface.md`). A shim at a
//! fixed path under `$GRIM_HOME` gives that: its *path* never moves, only its
//! contents, and no client hashes its contents.
//!
//! # The shim resolves grim by recorded absolute path, and stops there (W9)
//!
//! Decision D/A5 has the shim `exec` a recorded absolute `grim` "with a `$PATH`
//! lookup only as fallback". **This WP takes W9 explicitly and deletes that
//! fallback.** § Launcher's own next paragraph says `$PATH` is not an
//! alternative and names `PATH_add ./bin` as direnv's most common idiom: when
//! the recorded path is gone, a poisoned `$PATH` from the *client's* inherited
//! environment would choose the binary the **trusted shim** executes — CWE-426
//! reintroduced inside the one file the design treats as trusted. So a missing
//! recorded grim exits 0. Re-running `grim install` regenerates the shim, which
//! is the documented self-heal, so the fallback bought nothing a supported
//! command does not.
//!
//! # The registered string, byte for byte (B1, B2, B3, B8)
//!
//! ```sh
//! L='/abs/resolved/grim-home/hooks/bin/grim-hook'
//! [ -f "$L" ] && [ -x "$L" ] || exit 0
//! "$L" run --client <c> --event <E> --table '/abs/…/dispatch.json' --root <token>
//! s=$?
//! case "$s" in 0) exit 0 ;; <grim's own verdict codes for this client>) exit "$s" ;; *) exit 0 ;; esac
//! ```
//!
//! The `0) exit 0 ;;` arm is **not** redundant against the `*)` catch-all, and
//! an earlier revision of this doc comment dropped it — which is how a
//! byte-for-byte contract silently loses a byte. `Vendor::hook_registration`'s
//! doc is the authoritative spelling; this matches it. Keeping the arm means the
//! successful case is stated at the front of the `case` rather than reached by
//! falling through the two failure arms, so `verdict_exit_codes` can grow a row
//! without the reader having to re-derive what happens on 0.
//!
//! Every clause is an executed finding, not a preference:
//!
//! - **`L='…'`, POSIX single-quoted** (B2 · T3 · I1, I6). The quoting rule
//!   everyone reaches for is about the *use* site (`"$L"`); the **assignment**
//!   was a double-quoted literal, and a double-quoted literal still performs
//!   parameter expansion, command substitution and backticks. WP-P0 executed it
//!   under `dash` with a side-effect marker: a `$GRIM_HOME` containing `$(…)` or
//!   a backtick **ran the payload and the launcher never ran** — silent in both
//!   directions. `'` → `'\''` is correct for every hostile shape tested.
//! - **`[ -f "$L" ]` before `[ -x "$L" ]`** (B8 · I3). A directory carries the
//!   exec bit, so `-x` alone admits one and the spawn then exits **126**. On
//!   Copilot `preToolUse` any non-zero exit **denies the tool call**, so that
//!   row means grim denies every tool call in the session.
//! - **`"$L"` quoted at the use site.** Unquoted, with a space in the path,
//!   WP-B watched a planted executable at the word-split prefix run *instead of*
//!   the launcher.
//! - **No `exec`** (B8). `exec` forfeits the ability to distinguish "the
//!   launcher never ran" from "the launcher ran and returned a verdict", and the
//!   states it cannot distinguish are exactly the ones that deny on Copilot: a
//!   missing interpreter is 127, ENOEXEC is 126, mode `0100` is 2 — which is
//!   *Claude's deny code*. Cost: one extra `fork` per invocation, dwarfed by the
//!   spawn already in the design (WP-K's measurement must include it).
//! - **`--table '<abs>'`** (B1) — the table path is baked at install time and
//!   never recomputed from the environment at runtime. See
//!   [`super::hook_dispatch`].
//! - **`--root <token>`** (B3) — an opaque per-install token, never `global` and
//!   never an absolute workspace path.
//! - **Absolute, never `${GRIM_HOME:-…}`.** WP-B set a variable in each
//!   client's environment and watched it expand into the launcher's argv on
//!   claude, codex *and* copilot. An env-derived executed path in any
//!   registration is attacker-selectable (CWE-426, I1).
//! - **Never Copilot's exec-form `exec`/`args` field.** No shell ⇒ no guard, and
//!   a missing launcher becomes a spawn failure that `preToolUse` fails closed
//!   on — WP-B watched the tool call denied verbatim. `HookCommand::Argv` is
//!   consequently never constructed in v1.
//! - **The guard is emitted for claude too.** Claude is fail-open so its absence
//!   is not a Block, but without it the user gets a spurious
//!   `Hook command failed with code 127` in the transcript on *every* tool call
//!   while grim is not yet installed.
//!
//! POSIX-`sh` only, throughout: claude runs `/bin/sh`, copilot runs `bash`, and
//! codex runs the **user's** `$SHELL -lc` (WP-B § 4) — which is why a `fish` or
//! `nushell` user cannot execute this string at all. That is a real "hook
//! silently never fires" class, out of this fold and watchlisted (WP-M).

use std::io;
use std::path::{Path, PathBuf};

use crate::store::atomic_write::atomic_write;

use super::hook_dispatch::RootToken;

/// Directory under `$GRIM_HOME/hooks` holding the generated shim — and one of
/// the two reserved artifact names (`crate::oci::hook`).
pub const LAUNCHER_DIR: &str = "bin";

/// The shim's file name. Frozen: it is embedded in every already-written
/// registration, and no client re-reads a registration it has already trusted.
pub const LAUNCHER_FILE: &str = "grim-hook";

/// The shim's mode.
///
/// **Must be a separate `chmod`.** `atomic_write` caps modes at `0o644`
/// (`atomic_write.rs:40-50`), and D-12 records that no production code sets
/// `0o755` anywhere today — this is new machinery with no writer to extend. A
/// silent failure at that step leaves `[ -x ]` false and the hook **never
/// fires**, which is why S1 (verify the generated launcher at install time) is
/// carried as owed rather than assumed.
pub const LAUNCHER_MODE: u32 = 0o755;

/// `$GRIM_HOME/hooks/bin`.
pub fn launcher_dir(grim_home: &Path) -> PathBuf {
    super::hook_dispatch::hooks_dir(grim_home).join(LAUNCHER_DIR)
}

/// `$GRIM_HOME/hooks/bin/grim-hook` — the absolute literal every registration
/// embeds.
pub fn launcher_path(grim_home: &Path) -> PathBuf {
    launcher_dir(grim_home).join(LAUNCHER_FILE)
}

/// Per-client exit codes grim itself may return as a **deliberate verdict**,
/// keyed on `Vendor::name()` — the `case` allowlist of B8.
///
/// **Every v1 entry is empty, and that is a finding rather than an oversight.**
/// B8 writes the allowlist as "grim's own verdict codes for this client", and
/// C-004's `RESPONSE_PROJECTION` answers what those are: every verdict field on
/// every one of the twelve v1 `(client, event)` rows is a **JSON field on
/// stdout** (`decision`, `hookSpecificOutput.permissionDecision`), not an exit
/// code. Decision G says the launcher signals failure through its exit code
/// never, and a deny "per that vendor's blocking convention" — and for all three
/// v1 clients that convention is the JSON document. So there is no code to
/// preserve, and collapsing everything to 0 preserves grim's verdicts intact.
///
/// The three clients are listed present-and-empty on purpose: an absent client
/// would be indistinguishable from one nobody considered, and the next client
/// whose only verdict channel *is* an exit code (Claude's own documented
/// `exit 2` form, which grim does not project) needs one arm added here and
/// nothing else.
///
/// The `case` is still emitted when the list is empty — one string shape, one
/// code path, S3-pinnable byte for byte. Do not "simplify" it away: the shape is
/// what a future reader must not have to re-derive.
const VERDICT_EXIT_CODES: &[(&str, &[u8])] = &[("claude", &[]), ("codex", &[]), ("copilot", &[])];

/// The verdict-code allowlist for `client` (empty when the client declares
/// none, and empty for an unknown client — the fail-safe direction).
fn verdict_exit_codes(client: &str) -> &'static [u8] {
    match VERDICT_EXIT_CODES.iter().find(|(name, _)| *name == client) {
        Some((_, codes)) => codes,
        None => &[],
    }
}

/// A path or literal wrapped in POSIX single quotes, with embedded `'` escaped
/// the only way `sh` allows (`'\''`).
///
/// The **assignment**-site rule of B2, and the only correct embedding for every
/// hostile shape WP-P0 executed: space, `'`, `${…}`, `$(…)`, backtick, newline,
/// `;`, `\`. Implemented rather than stubbed because this quoting *is* the
/// C-018b argument — a body-less version would leave the constraint unexpressed.
///
/// A newline still round-trips through this function correctly at the shell
/// level, but no vendor's JSON-plus-shell round trip has a correct quoting for
/// one, so [`registered_command`] refuses a path containing one outright rather
/// than relying on this.
pub fn posix_single_quote(value: &str) -> String {
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

/// A literal wrapped in PowerShell single quotes (`'` → `''`).
///
/// PowerShell's single-quoted string is verbatim — no expansion, no escapes
/// except the doubled quote — so it is the same property [`posix_single_quote`]
/// buys, in the other dialect.
fn powershell_single_quote(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('\'');
    for c in value.chars() {
        if c == '\'' {
            out.push_str("''");
        } else {
            out.push(c);
        }
    }
    out.push('\'');
    out
}

/// Why a command string could not be generated.
///
/// One variant, and it is a **refusal to arm**, not a failure: the registrar
/// maps it to `ArmRefusal::LauncherPathControlChar` and reports `not-armed`
/// (C-017 cause 3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandRefusal {
    /// The resolved launcher or table path contains a newline or another control
    /// character (B2).
    ///
    /// No vendor's JSON-plus-shell round trip has a correct quoting for a
    /// newline, and no legitimate path needs one — so grim refuses rather than
    /// writing a registration whose behaviour it cannot predict. Note this is a
    /// property of `$GRIM_HOME`, which is environment-derived, which is exactly
    /// why C-018b had to be widened from "no publisher-controlled value" to "no
    /// value grim did not itself choose".
    ControlCharacterInPath,
}

impl CommandRefusal {
    /// The reason phrase, library style (lowercase, no trailing punctuation).
    pub fn reason(self) -> &'static str {
        match self {
            Self::ControlCharacterInPath => "the resolved launcher or table path contains a control character",
        }
    }
}

impl std::fmt::Display for CommandRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.reason())
    }
}

/// Everything the command string interpolates — and nothing else can reach it.
///
/// C-018b as a type: the struct's fields *are* the closed set of values that
/// appear in the generated line. `matcher`, `hook.id`, the artifact name and
/// every vendor override are absent by construction, so "no value grim did not
/// itself choose is interpolated" is checkable by reading this declaration
/// rather than by auditing a format string.
#[derive(Debug, Clone, Copy)]
pub struct CommandSpec<'a> {
    /// The resolved absolute launcher path ([`launcher_path`]).
    pub launcher: &'a Path,
    /// The resolved absolute dispatch-table path
    /// ([`super::hook_dispatch::dispatch_path`]) — baked in, never recomputed
    /// from the environment at runtime (B1).
    pub table: &'a Path,
    /// `Vendor::name()`.
    pub client: &'a str,
    /// The client's own spelling of the firing event
    /// (`Vendor::hook_event_name` — PascalCase on all three v1 clients, and
    /// **mandatory** PascalCase on copilot, whose camelCase dialect silently
    /// skips grim's matchers).
    pub event: &'a str,
    /// The opaque root token (B3).
    pub root: &'a RootToken,
}

impl CommandSpec<'_> {
    /// Whether either resolved path carries a control character (B2).
    fn has_control_character(&self) -> bool {
        [self.launcher, self.table]
            .iter()
            .any(|p| path_is_representable(p).is_err())
    }
}

/// Whether `path` can be embedded in a registration at all (B2).
///
/// Split out of [`CommandSpec::has_control_character`] because
/// [`super::hook_registrar::arming_refusal`] must answer C-017 cause 3
/// **read-only**, before a client, an event or a root token exist — so it has
/// the two paths but no [`CommandSpec`] to ask.
///
/// # Errors
///
/// [`CommandRefusal::ControlCharacterInPath`].
pub fn path_is_representable(path: &Path) -> Result<(), CommandRefusal> {
    if path.to_string_lossy().chars().any(char::is_control) {
        return Err(CommandRefusal::ControlCharacterInPath);
    }
    Ok(())
}

/// The `case`/`switch` arms that pass `client`'s own verdict codes through, in
/// the dialect's own syntax.
///
/// Empty for all three v1 clients — see [`VERDICT_EXIT_CODES`]. Rendered from
/// the same table for both dialects so a code added there cannot reach one
/// registration and miss the other.
fn verdict_arms(client: &str, render: impl Fn(u8) -> String) -> String {
    verdict_exit_codes(client).iter().copied().map(render).collect()
}

/// Generate the POSIX-`sh` registration string for `spec` — **the generator
/// half of C-008/C-018b**.
///
/// The *assembly* site (`HookEntry` → `HookRegistration`) is
/// `Vendor::hook_registration`; this is the string it composes, kept here
/// because the string is a property of the launcher, not of any vendor. There is
/// exactly one generator, so C-018b's pinning test — build a registration from a
/// metacharacter-laden manifest and assert the command string is byte-identical
/// to the metacharacter-free case — has a single thing to pin.
///
/// # Errors
///
/// [`CommandRefusal::ControlCharacterInPath`]; the caller reports `not-armed`
/// and writes no registration.
// `cfg_attr(not(test))`: this module's own tests exercise it, so an
// unconditional `expect` would be unfulfilled in the test profile.
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "WP-I stub: consumed by Vendor::hook_registration (WP-F implement) and WP-J2's install branch. REMOVAL TRIGGER: delete this attribute when that caller lands"
    )
)]
pub fn registered_command(spec: &CommandSpec<'_>) -> Result<String, CommandRefusal> {
    if spec.has_control_character() {
        return Err(CommandRefusal::ControlCharacterInPath);
    }
    let launcher = posix_single_quote(&spec.launcher.to_string_lossy());
    let table = posix_single_quote(&spec.table.to_string_lossy());
    let arms = verdict_arms(spec.client, |code| format!("{code}) exit \"$s\" ;; "));
    // No trailing newline: the value goes into a JSON string field, and a
    // trailing newline there is a byte a client's own trust hash would carry
    // around for nothing.
    Ok(format!(
        "L={launcher}\n\
         [ -f \"$L\" ] && [ -x \"$L\" ] || exit 0\n\
         \"$L\" run --client {client} --event {event} --table {table} --root {root}\n\
         s=$?\n\
         case \"$s\" in 0) exit 0 ;; {arms}*) exit 0 ;; esac",
        client = spec.client,
        event = spec.event,
        root = spec.root.as_str(),
    ))
}

/// The PowerShell form of [`registered_command`], for the vendors with a
/// separate Windows field (codex `commandWindows`, copilot `powershell`).
///
/// Same clause order, same properties, other dialect: a verbatim single-quoted
/// assignment, a leaf-file test (Windows has no exec bit, so `Test-Path
/// -PathType Leaf` is the whole guard `[ -f ] && [ -x ]` collapses to), no
/// `exec` equivalent, and the same collapse of every non-verdict code to 0.
///
/// **Runtime-unverified.** WP-B confirmed both vendors *accept* the field in
/// their schema and fired the POSIX hook on Linux with it present; no Windows
/// host was available, so the string's behaviour there is untested. Watchlisted
/// (WP-M) rather than trusted.
///
/// # Errors
///
/// As [`registered_command`].
// `cfg_attr(not(test))`: this module's own tests exercise it, so an
// unconditional `expect` would be unfulfilled in the test profile.
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "WP-I stub: consumed by Vendor::hook_registration (WP-F implement) and WP-J2's install branch. REMOVAL TRIGGER: delete this attribute when that caller lands"
    )
)]
pub fn registered_command_powershell(spec: &CommandSpec<'_>) -> Result<String, CommandRefusal> {
    if spec.has_control_character() {
        return Err(CommandRefusal::ControlCharacterInPath);
    }
    let launcher = powershell_single_quote(&spec.launcher.to_string_lossy());
    let table = powershell_single_quote(&spec.table.to_string_lossy());
    let arms = verdict_arms(spec.client, |code| format!("{code} {{ exit $s }} "));
    // `-LiteralPath` matters as much as the quoting does: the plain `-Path`
    // parameter treats `[`, `]`, `*` and `?` as wildcards, so a launcher under a
    // directory containing one would be tested as a pattern rather than as a
    // path — the Windows analogue of the word-splitting hole B2 closes.
    Ok(format!(
        "$L = {launcher}\n\
         if (-not (Test-Path -LiteralPath $L -PathType Leaf)) {{ exit 0 }}\n\
         & $L run --client {client} --event {event} --table {table} --root {root}\n\
         $s = $LASTEXITCODE\n\
         switch ($s) {{ 0 {{ exit 0 }} {arms}default {{ exit 0 }} }}",
        client = spec.client,
        event = spec.event,
        root = spec.root.as_str(),
    ))
}

/// The shim's contents for a grim binary at `grim_binary`.
///
/// Body shape:
///
/// ```sh
/// #!/bin/sh
/// # generated by grim — do not edit; `grim install` regenerates it
/// G='<recorded absolute grim>'
/// [ -f "$G" ] && [ -x "$G" ] || exit 0
/// exec "$G" hook "$@"
/// ```
///
/// `exec` **is** correct here, and the contrast with [`registered_command`] is
/// deliberate: inside the shim the goal is to *become* grim, and a failed `exec`
/// exits 126/127 into the registration's own remap, which collapses it to 0. In
/// the registration `exec` would forfeit the distinction B8 needs.
///
/// No `$PATH` fallback (W9) — see the module doc.
///
/// # Errors
///
/// [`CommandRefusal::ControlCharacterInPath`] when `grim_binary` carries a
/// control character.
pub fn shim_body(grim_binary: &Path) -> Result<String, CommandRefusal> {
    path_is_representable(grim_binary)?;
    let grim = posix_single_quote(&grim_binary.to_string_lossy());
    // Trailing newline, unlike the registration: this one is a file, and a
    // script without a final newline is a POSIX incomplete line.
    Ok(format!(
        "#!/bin/sh\n\
         # generated by grim — do not edit; `grim install` regenerates it\n\
         G={grim}\n\
         [ -f \"$G\" ] && [ -x \"$G\" ] || exit 0\n\
         exec \"$G\" hook \"$@\"\n"
    ))
}

/// What [`generate`] did, for one `tracing` line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LauncherWrite {
    /// The shim on disk already had these bytes and this mode.
    Unchanged,
    /// Written (or rewritten) and `chmod`ed.
    Written,
}

/// Why the launcher could not be generated.
#[derive(Debug)]
pub enum LauncherError {
    /// The path is unrepresentable in a registration (B2).
    Refused(CommandRefusal),
    /// An I/O failure creating `$GRIM_HOME/hooks/bin/`, writing the shim, or
    /// setting its mode.
    Io(io::Error),
}

impl std::fmt::Display for LauncherError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Refused(r) => f.write_str(r.reason()),
            Self::Io(e) => write!(f, "launcher I/O failure: {e}"),
        }
    }
}

impl std::error::Error for LauncherError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Refused(_) => None,
            Self::Io(e) => Some(e),
        }
    }
}

/// Write the shim at [`launcher_path`], self-healing and byte-stable.
///
/// Idempotent: identical bytes and mode ⇒ [`LauncherWrite::Unchanged`] and no
/// write, so re-materialization leaves `status` not-modified (Principle 9's
/// self-heal obligation for a renderer change).
///
/// `grim_binary` is the absolute path of the running binary — recorded into the
/// shim so the shim never consults `$PATH` (W9). The caller resolves it;
/// `std::env::current_exe` is the only source, and it is deliberately read
/// outside this function so the whole generator is a pure function of its
/// arguments and hermetically testable.
///
/// **`0o755` is a separate `chmod`** after the atomic write, because
/// `atomic_write` caps at `0o644`. The directory is created `0o700` (W3).
///
/// # Errors
///
/// [`LauncherError::Refused`] for a control character in either path;
/// [`LauncherError::Io`] for any filesystem failure — including a failed
/// `chmod`, which must **never** be ignored: silently leaving the shim
/// non-executable makes `[ -x ]` false and the hook never fires (S1).
pub fn generate(grim_home: &Path, grim_binary: &Path) -> Result<LauncherWrite, LauncherError> {
    let body = shim_body(grim_binary).map_err(LauncherError::Refused)?;
    let path = launcher_path(grim_home);
    // The registration embeds this path, so a path grim could not quote must
    // refuse here too rather than at the assembly site — otherwise the shim is
    // written and only the registration is missing, which is armed-looking and
    // inert.
    path_is_representable(&path).map_err(LauncherError::Refused)?;

    if shim_is_current(&path, &body) {
        return Ok(LauncherWrite::Unchanged);
    }

    let dir = launcher_dir(grim_home);
    std::fs::create_dir_all(&dir).map_err(LauncherError::Io)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // `hooks/` and `hooks/bin/` both, and the parent first: narrowing the
        // leaf while its parent stays group-writable buys nothing (W3).
        for dir in [super::hook_dispatch::hooks_dir(grim_home), dir.clone()] {
            std::fs::set_permissions(
                &dir,
                std::fs::Permissions::from_mode(super::hook_dispatch::HOOKS_DIR_MODE),
            )
            .map_err(LauncherError::Io)?;
        }
    }

    atomic_write(&path, body.as_bytes()).map_err(LauncherError::Io)?;
    // `atomic_write` caps at `0o644`, so the exec bit is a separate step — and
    // its failure must never be ignored: a non-executable shim makes `[ -x ]`
    // false and the hook never fires (S1).
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(LAUNCHER_MODE)).map_err(LauncherError::Io)?;
    }
    Ok(LauncherWrite::Written)
}

/// Whether the shim on disk already has exactly these bytes **and** the exec
/// mode — the idempotence test [`generate`] returns
/// [`LauncherWrite::Unchanged`] on.
///
/// Both halves are required. Bytes alone would leave a shim whose `chmod` failed
/// on an earlier run reported as up to date forever, which is the one failure
/// mode that makes a hook silently never fire.
fn shim_is_current(path: &Path, body: &str) -> bool {
    let Ok(existing) = std::fs::read(path) else {
        return false;
    };
    if existing != body.as_bytes() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        match std::fs::metadata(path) {
            Ok(meta) => meta.permissions().mode() & 0o777 == LAUNCHER_MODE,
            Err(_) => false,
        }
    }
    #[cfg(not(unix))]
    {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::install::hook_dispatch::{RootScope, dispatch_path, root_token};

    fn token() -> (tempfile::TempDir, RootToken) {
        let home = tempfile::tempdir().unwrap();
        let token = root_token(home.path(), RootScope::Global).unwrap();
        (home, token)
    }

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

    #[test]
    fn powershell_quoting_doubles_the_only_escapable_character() {
        assert_eq!(powershell_single_quote(r"C:\Users\dev\g"), r"'C:\Users\dev\g'");
        assert_eq!(powershell_single_quote("C:\\o'brien\\g"), "'C:\\o''brien\\g'");
        assert_eq!(powershell_single_quote("$env:HOME"), "'$env:HOME'");
    }

    #[test]
    fn the_registered_string_is_the_corrected_five_line_form() {
        let (home, token) = token();
        let launcher = launcher_path(home.path());
        let table = dispatch_path(home.path());
        let command = registered_command(&CommandSpec {
            launcher: &launcher,
            table: &table,
            client: "copilot",
            event: "preToolUse",
            root: &token,
        })
        .unwrap();

        assert_eq!(
            command,
            format!(
                "L='{launcher}'\n\
                 [ -f \"$L\" ] && [ -x \"$L\" ] || exit 0\n\
                 \"$L\" run --client copilot --event preToolUse --table '{table}' --root {token}\n\
                 s=$?\n\
                 case \"$s\" in 0) exit 0 ;; *) exit 0 ;; esac",
                launcher = launcher.display(),
                table = table.display(),
            )
        );
        // The five findings, asserted individually so a regression names itself.
        assert!(command.starts_with("L='"), "B2: single-quoted assignment");
        assert!(command.contains("[ -f \"$L\" ] && [ -x \"$L\" ]"), "B8: -f before -x");
        assert!(!command.contains("exec "), "B8: no exec in the registration");
        assert!(command.contains("--table '"), "B1: argv-located table");
        assert!(!command.contains("--root global"), "B3: never the literal root");
        assert!(!command.contains("$GRIM_HOME"), "never environment-derived");
    }

    #[test]
    fn a_metacharacter_laden_grim_home_changes_only_the_quoted_path() {
        // C-018b's pinning shape: the *structure* of the line is identical, and
        // the hostile bytes appear only inside the single quotes.
        let hostile = Path::new("/tmp/$(touch pwned)/hooks/bin/grim-hook");
        let table = Path::new("/tmp/$(touch pwned)/hooks/dispatch.json");
        let (_home, token) = token();
        let command = registered_command(&CommandSpec {
            launcher: hostile,
            table,
            client: "claude",
            event: "PreToolUse",
            root: &token,
        })
        .unwrap();

        let lines: Vec<&str> = command.lines().collect();
        assert_eq!(lines.len(), 5);
        assert_eq!(lines[0], "L='/tmp/$(touch pwned)/hooks/bin/grim-hook'");
        assert_eq!(lines[1], "[ -f \"$L\" ] && [ -x \"$L\" ] || exit 0");
        assert_eq!(lines[3], "s=$?");
        assert_eq!(lines[4], "case \"$s\" in 0) exit 0 ;; *) exit 0 ;; esac");
    }

    #[test]
    fn a_control_character_in_either_path_refuses_rather_than_quotes() {
        let (_home, token) = token();
        for (launcher, table) in [
            (Path::new("/tmp/a\nb/grim-hook"), Path::new("/tmp/dispatch.json")),
            (Path::new("/tmp/grim-hook"), Path::new("/tmp/a\tb/dispatch.json")),
        ] {
            let spec = CommandSpec {
                launcher,
                table,
                client: "claude",
                event: "PreToolUse",
                root: &token,
            };
            assert_eq!(registered_command(&spec), Err(CommandRefusal::ControlCharacterInPath));
            assert_eq!(
                registered_command_powershell(&spec),
                Err(CommandRefusal::ControlCharacterInPath)
            );
        }
    }

    #[test]
    fn the_powershell_form_keeps_the_same_clause_order() {
        let (home, token) = token();
        let launcher = launcher_path(home.path());
        let table = dispatch_path(home.path());
        let command = registered_command_powershell(&CommandSpec {
            launcher: &launcher,
            table: &table,
            client: "codex",
            event: "PreToolUse",
            root: &token,
        })
        .unwrap();

        let lines: Vec<&str> = command.lines().collect();
        assert_eq!(lines.len(), 5);
        assert!(lines[0].starts_with("$L = '"));
        assert_eq!(
            lines[1],
            "if (-not (Test-Path -LiteralPath $L -PathType Leaf)) { exit 0 }"
        );
        assert!(lines[2].starts_with("& $L run --client codex --event PreToolUse --table '"));
        assert_eq!(lines[3], "$s = $LASTEXITCODE");
        assert_eq!(lines[4], "switch ($s) { 0 { exit 0 } default { exit 0 } }");
    }

    #[test]
    fn every_v1_client_has_an_empty_verdict_allowlist() {
        // D-I-8: grim projects every verdict as a JSON field, so there is no
        // exit code to preserve. Present-and-empty, never absent.
        for client in ["claude", "codex", "copilot"] {
            assert!(verdict_exit_codes(client).is_empty(), "{client}");
        }
        assert!(verdict_exit_codes("cursor").is_empty(), "unknown clients fail safe");
    }

    #[test]
    fn the_shim_execs_the_recorded_path_and_never_consults_path() {
        let body = shim_body(Path::new("/usr/local/bin/grim")).unwrap();
        assert_eq!(
            body,
            "#!/bin/sh\n\
             # generated by grim — do not edit; `grim install` regenerates it\n\
             G='/usr/local/bin/grim'\n\
             [ -f \"$G\" ] && [ -x \"$G\" ] || exit 0\n\
             exec \"$G\" hook \"$@\"\n"
        );
        // W9: no `$PATH` fallback, and `exec` *is* correct here.
        assert!(!body.contains("command -v"));
        assert!(!body.contains("PATH"));
        assert!(body.contains("exec \"$G\""));
    }

    #[test]
    fn generate_is_idempotent_and_leaves_the_shim_executable() {
        let home = tempfile::tempdir().unwrap();
        let grim = home.path().join("bin/grim");

        assert_eq!(generate(home.path(), &grim).unwrap(), LauncherWrite::Written);
        let path = launcher_path(home.path());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), shim_body(&grim).unwrap());
        assert_eq!(generate(home.path(), &grim).unwrap(), LauncherWrite::Unchanged);

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                LAUNCHER_MODE
            );
            assert_eq!(
                std::fs::metadata(launcher_dir(home.path()))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                crate::install::hook_dispatch::HOOKS_DIR_MODE
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_shim_whose_exec_bit_was_lost_is_rewritten_not_reported_current() {
        use std::os::unix::fs::PermissionsExt;
        let home = tempfile::tempdir().unwrap();
        let grim = home.path().join("bin/grim");
        generate(home.path(), &grim).unwrap();

        let path = launcher_path(home.path());
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();

        // Bytes alone would say "current" and the hook would never fire (S1).
        assert_eq!(generate(home.path(), &grim).unwrap(), LauncherWrite::Written);
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            LAUNCHER_MODE
        );
    }

    #[test]
    fn generate_refuses_a_grim_path_it_could_not_quote() {
        let home = tempfile::tempdir().unwrap();
        let err = generate(home.path(), Path::new("/usr/bin/gr\nim")).unwrap_err();
        assert!(matches!(
            err,
            LauncherError::Refused(CommandRefusal::ControlCharacterInPath)
        ));
        assert!(!launcher_path(home.path()).exists());
    }
}
