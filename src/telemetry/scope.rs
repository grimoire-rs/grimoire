// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! Recording the scope the user asked for.

use std::path::Path;

use crate::config::scope::ConfigScope;
use crate::telemetry::Telemetry;

impl Telemetry {
    /// Records the scope a command resolved, if it is the one the user asked for.
    ///
    /// `ctx_global` is the invocation's `--global`; `global` and `seed` are the
    /// arguments of the `resolve_in` call being recorded. Only a call that
    /// repeats the invocation's own `--global` and carries no workspace seed
    /// counts: an internal side resolve (a global command that also reads the
    /// project, `export`'s project lookup) must not turn `grim.scope` or the
    /// `vcs.*` gate. The first such call wins. `mcp`/`tui` record nothing.
    pub fn record_scope(
        &self,
        ctx_global: bool,
        global: bool,
        seed: Option<&Path>,
        scope: ConfigScope,
        workspace: &Path,
    ) {
        if global != ctx_global || seed.is_some() {
            return;
        }
        self.record(|state| {
            if state.scope.is_none() {
                state.scope = Some(scope);
                state.workspace = Some(workspace.to_path_buf());
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::exit_code::ExitCode;
    use std::path::PathBuf;

    fn armed() -> Telemetry {
        Telemetry::armed_for_test(PathBuf::from("/grim-home"))
    }

    #[test]
    fn records_the_requested_scope_and_its_workspace() {
        let t = armed();
        t.record_scope(false, false, None, ConfigScope::Project, Path::new("/work/proj"));
        let s = t.snapshot().unwrap();
        assert_eq!(s.scope, Some(ConfigScope::Project));
        assert_eq!(s.workspace.as_deref(), Some(Path::new("/work/proj")));
    }

    #[test]
    fn a_global_command_that_also_resolves_a_project_reports_global_with_no_project_workspace() {
        let t = armed();
        // `grim --global export …`: the project lookup happens first, internally.
        t.record_scope(true, false, None, ConfigScope::Project, Path::new("/work/proj"));
        t.record_scope(true, true, None, ConfigScope::Global, Path::new("/grim-home"));
        let s = t.snapshot().unwrap();
        assert_eq!(s.scope, Some(ConfigScope::Global));
        assert_eq!(s.workspace.as_deref(), Some(Path::new("/grim-home")));
    }

    #[test]
    fn a_seeded_resolve_is_never_the_requested_scope() {
        let t = armed();
        t.record_scope(
            false,
            false,
            Some(Path::new("/elsewhere")),
            ConfigScope::Project,
            Path::new("/elsewhere"),
        );
        assert!(t.snapshot().unwrap().scope.is_none());
    }

    #[test]
    fn the_first_requested_resolution_wins() {
        let t = armed();
        t.record_scope(false, false, None, ConfigScope::Project, Path::new("/first"));
        t.record_scope(false, false, None, ConfigScope::Project, Path::new("/second"));
        assert_eq!(t.snapshot().unwrap().workspace.as_deref(), Some(Path::new("/first")));
    }

    #[test]
    fn record_scope_noop_after_start_mcp() {
        let t = armed();
        t.start(&crate::telemetry::parsed_command(&["mcp"]));
        t.record_scope(false, false, None, ConfigScope::Project, Path::new("/work/proj"));
        let s = t.snapshot().unwrap();
        assert!(s.long_lived);
        assert!(s.scope.is_none() && s.workspace.is_none());
    }

    #[test]
    fn record_scope_noop_after_start_tui() {
        let t = armed();
        t.start(&crate::telemetry::parsed_command(&["tui"]));
        t.record_scope(false, false, None, ConfigScope::Project, Path::new("/work/proj"));
        assert!(t.snapshot().unwrap().scope.is_none());
    }

    #[test]
    fn an_ordinary_command_is_not_long_lived() {
        let t = armed();
        t.start(&crate::telemetry::parsed_command(&["lock"]));
        assert!(!t.snapshot().unwrap().long_lived);
    }

    #[test]
    fn nothing_is_recorded_once_finish_has_run() {
        let t = armed();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(t.finish_with(&Ok(ExitCode::Success), |_| None));
        t.record_scope(false, false, None, ConfigScope::Project, Path::new("/late"));
        assert!(t.snapshot().unwrap().scope.is_none());
    }
}
