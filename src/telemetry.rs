// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! Opt-in OTel usage telemetry.
//!
//! One OTLP/HTTP request per CLI invocation: a `grim.command` event plus one
//! `grim.artifact.changed` per changed artifact. Nothing here runs unless the
//! operator sets `GRIM_TELEMETRY`; an unarmed [`Telemetry`] holds no state and
//! every method on it returns immediately. The exemption this module rests on
//! is `adr_otel_usage_telemetry.md`.
//!
//! The handle lives on [`crate::context::Context`], which is built after
//! `grim hook run` has already returned, so the hook path can never reach it.

use std::cell::Cell;
use std::future::Future;
use std::panic::PanicHookInfo;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::{Arc, Mutex, MutexGuard, Once};
use std::task::{Context, Poll};
use std::time::Instant;

use crate::Command;
use crate::cli::exit_code::ExitCode;
use crate::config::scope::ConfigScope;
use crate::install::client_target::ClientTarget;

pub mod command_name;
pub mod config;
pub mod export;
pub mod identity;
pub mod payload;
pub mod record;
pub mod redact;
pub mod scope;
pub mod vcs;

use record::ArtifactEvent;

/// Command name reported when `start` never saw a parsed command.
pub(crate) const UNKNOWN_COMMAND: &str = "unknown";

/// Commands that stay alive for a whole session and so report nothing but
/// the closing `grim.command` event.
const LONG_LIVED: [&str; 2] = ["mcp", "tui"];

/// `GRIM_LOG` target every telemetry `tracing` call passes explicitly; the
/// default would be the crate name (`grimoire::telemetry`).
pub(crate) const LOG_TARGET: &str = "grim::telemetry";

/// What one invocation has recorded so far.
#[derive(Debug, Clone)]
pub(crate) struct State {
    pub grim_home: PathBuf,
    /// `GRIM_TELEMETRY_REPOSITORY` as read when the handle was built.
    pub repository: bool,
    pub started: Instant,
    pub command: Option<(&'static str, Option<&'static str>)>,
    /// Set by `start` for `mcp`/`tui`; every `record_*` returns early on it.
    pub long_lived: bool,
    /// Set once `finish` has taken its snapshot; later records are dropped.
    pub finished: bool,
    pub scope: Option<ConfigScope>,
    pub workspace: Option<PathBuf>,
    pub clients: Option<Vec<ClientTarget>>,
    pub detected: Option<Vec<ClientTarget>>,
    pub artifacts: Vec<ArtifactEvent>,
}

impl State {
    pub(crate) fn new(grim_home: PathBuf, repository: bool) -> Self {
        Self {
            grim_home,
            repository,
            started: Instant::now(),
            command: None,
            long_lived: false,
            finished: false,
            scope: None,
            workspace: None,
            clients: None,
            detected: None,
            artifacts: Vec::new(),
        }
    }
}

/// Whether telemetry is armed: the switch is on and no opt-out applies.
///
/// A pure function of four booleans so the 16-row truth table needs no
/// process-environment mutation.
pub(crate) fn armed(switch: bool, sdk_disabled: bool, do_not_track: bool, offline: bool) -> bool {
    switch && !sdk_disabled && !do_not_track && !offline
}

/// Per-invocation telemetry handle; cheap to clone, shared between clones.
#[derive(Clone, Default)]
pub struct Telemetry {
    state: Option<Arc<Mutex<State>>>,
}

impl std::fmt::Debug for Telemetry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(if self.state.is_some() {
            "Telemetry(armed)"
        } else {
            "Telemetry(unarmed)"
        })
    }
}

impl Telemetry {
    /// Decides the gate from the environment: four reads, no I/O, no OTel type.
    pub fn from_env(offline: bool, grim_home: &Path) -> Self {
        if !armed(
            crate::env::telemetry(),
            crate::env::otel_sdk_disabled(),
            crate::env::do_not_track(),
            offline,
        ) {
            return Self::default();
        }
        Self::armed_with(State::new(grim_home.to_path_buf(), crate::env::telemetry_repository()))
    }

    fn armed_with(state: State) -> Self {
        install_quiet_panic_hook();
        Self {
            state: Some(Arc::new(Mutex::new(state))),
        }
    }

    /// Notes which command is running and the clock's start.
    pub fn start(&self, command: &Command) {
        self.update(|state| {
            let (name, subcommand) = command_name::command_name(Some(command));
            state.started = Instant::now();
            state.command = Some((name, subcommand));
            state.long_lived = LONG_LIVED.contains(&name);
        });
    }

    /// Sends the invocation's events. Never changes the outcome: everything
    /// after the armed check runs in its own task, and a panic in it ends as a
    /// debug line.
    pub async fn finish(&self, outcome: &anyhow::Result<ExitCode>) {
        self.finish_with(outcome, env_lookup).await;
    }

    async fn finish_with(&self, outcome: &anyhow::Result<ExitCode>, lookup: fn(&str) -> Option<String>) {
        let Some(shared) = &self.state else { return };
        let snapshot = {
            let mut state = lock(shared);
            if state.finished {
                return;
            }
            state.finished = true;
            state.clone()
        };
        let (exit_code, error_type) = payload::exit_info(outcome);
        let outcome = payload::Outcome {
            duration: snapshot.started.elapsed(),
            exit_code,
            error_type,
            ci: crate::env::ci(),
        };
        isolated(deliver(snapshot, outcome, lookup)).await;
    }

    /// Applies `edit` unless the invocation already finished.
    pub(crate) fn update(&self, edit: impl FnOnce(&mut State)) {
        if let Some(shared) = &self.state {
            let mut state = lock(shared);
            if !state.finished {
                edit(&mut state);
            }
        }
    }

    /// Like [`Self::update`], but also a no-op for `mcp`/`tui`, which record
    /// nothing during the session.
    pub(crate) fn record(&self, edit: impl FnOnce(&mut State)) {
        self.update(|state| {
            if !state.long_lived {
                edit(state);
            }
        });
    }

    #[cfg(test)]
    pub(crate) fn armed_for_test(grim_home: PathBuf) -> Self {
        Self::armed_with(State::new(grim_home, false))
    }

    #[cfg(test)]
    pub(crate) fn snapshot(&self) -> Option<State> {
        self.state.as_ref().map(|shared| lock(shared).clone())
    }
}

#[cfg(test)]
pub(crate) fn parsed_command(args: &[&str]) -> Command {
    use clap::Parser;
    let argv = std::iter::once("grim").chain(args.iter().copied());
    crate::Cli::try_parse_from(argv)
        .expect("test argv parses")
        .command
        .expect("test argv names a command")
}

/// Locks `mutex`, ignoring poison: telemetry state is only ever observed, never trusted to be consistent.
pub(crate) fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Runs `task` on its own tokio task so a panic inside it ends as a debug line
/// instead of unwinding into the command's exit path or printing on stderr.
async fn isolated(task: impl Future<Output = ()> + Send + 'static) {
    if let Err(err) = tokio::spawn(Quiet(Box::pin(task))).await {
        tracing::debug!(target: LOG_TARGET, "telemetry task failed: {err}");
    }
}

thread_local! {
    /// True while a telemetry task is being polled on this thread.
    static QUIET: Cell<bool> = const { Cell::new(false) };
}

/// Polls the inner future with [`QUIET`] set, so the panic hook skips its panics.
struct Quiet<F>(Pin<Box<F>>);

impl<F: Future> Future for Quiet<F> {
    type Output = F::Output;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<F::Output> {
        struct Reset;
        impl Drop for Reset {
            fn drop(&mut self) {
                QUIET.set(false);
            }
        }
        QUIET.set(true);
        // Resets on unwind too: a flag left set would silence every later
        // panic on this worker thread, grim's own included. Owned by
        // a_telemetry_panic_skips_the_previous_hook_and_any_other_panic_reaches_it.
        let _reset = Reset;
        self.0.as_mut().poll(cx)
    }
}

type PanicHook = Box<dyn Fn(&PanicHookInfo<'_>) + Send + Sync + 'static>;

/// Wraps `previous` so it is skipped while [`QUIET`] is set.
fn quiet(previous: PanicHook) -> PanicHook {
    Box::new(move |info| {
        if !QUIET.get() {
            previous(info);
        }
    })
}

/// Installs [`quiet`] over the current panic hook, once per process.
// ponytail: the hook is process-wide but only silent during telemetry polls;
// a per-task hook does not exist in std, so this is the narrowest scope.
fn install_quiet_panic_hook() {
    static INSTALL: Once = Once::new();
    INSTALL.call_once(|| std::panic::set_hook(quiet(std::panic::take_hook())));
}

fn env_lookup(key: &str) -> Option<String> {
    std::env::var(key).ok()
}

/// The post-gate pipeline, in the order the ADR documents.
async fn deliver(state: State, outcome: payload::Outcome, lookup: fn(&str) -> Option<String>) {
    let installation_id = identity::installation_id(&state.grim_home).await;

    let vcs = match (&state.workspace, state.scope) {
        (Some(workspace), Some(ConfigScope::Project)) if state.repository => {
            vcs::derive(workspace, crate::env::home_dir().as_deref()).await
        }
        _ => None,
    };

    let overrides = config::resource_overrides(&lookup);
    let resource = payload::resource(installation_id.as_deref(), vcs.as_ref(), overrides);
    let payload = payload::build(resource, &state, &outcome);
    tracing::debug!(target: LOG_TARGET, "telemetry payload {}", payload.to_json_line());

    let export_config = match config::from_env(&lookup) {
        Ok(cfg) => cfg,
        Err(skip) => {
            tracing::debug!(target: LOG_TARGET, "telemetry not sent: {skip}");
            return;
        }
    };
    let sent = tokio::time::timeout(export_config.timeout, export::send(&export_config, &payload)).await;
    let failure = match sent {
        Ok(Ok(())) => return,
        Ok(Err(err)) => err.to_string(),
        Err(_) => format!("timed out after {} ms", export_config.timeout.as_millis()),
    };
    tracing::debug!(
        target: LOG_TARGET,
        "telemetry export to {} failed: {failure}",
        redact::url(export_config.endpoint.as_str())
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every combination of the four inputs: armed exactly when the switch is
    /// on and all three opt-outs are off.
    #[test]
    fn gate_truth_table_arms_only_for_switch_without_any_opt_out() {
        let mut armed_rows = 0;
        for bits in 0u8..16 {
            let (switch, sdk, dnt, offline) = (bits & 1 != 0, bits & 2 != 0, bits & 4 != 0, bits & 8 != 0);
            let expected = switch && !sdk && !dnt && !offline;
            assert_eq!(
                armed(switch, sdk, dnt, offline),
                expected,
                "switch={switch} sdk_disabled={sdk} dnt={dnt} offline={offline}"
            );
            armed_rows += usize::from(expected);
        }
        assert_eq!(armed_rows, 1, "exactly one of the 16 rows arms");
    }

    #[test]
    fn unarmed_handle_holds_no_state_and_ignores_every_call() {
        let t = Telemetry::default();
        t.start(&parsed_command(&["lock"]));
        t.record(|s| s.long_lived = true);
        assert!(t.snapshot().is_none());
    }

    /// Captures every `tracing` event emitted inside `f` on this thread.
    fn capture_logs(f: impl FnOnce()) -> String {
        #[derive(Clone, Default)]
        struct Sink(Arc<Mutex<Vec<u8>>>);
        impl std::io::Write for Sink {
            fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
                self.0.lock().unwrap().extend_from_slice(buf);
                Ok(buf.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        crate::log_switch::tracing_capture::arm();
        let sink = Sink::default();
        let writer = sink.clone();
        let subscriber = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::DEBUG)
            .with_ansi(false)
            .with_target(true)
            .with_writer(move || writer.clone())
            .finish();
        let guard = tracing::subscriber::set_default(subscriber);
        f();
        drop(guard);
        String::from_utf8(sink.0.lock().unwrap().clone()).unwrap()
    }

    /// Finishes with an empty environment, so no ambient endpoint is ever
    /// contacted from a unit test.
    fn finish_blocking(t: &Telemetry, outcome: &anyhow::Result<ExitCode>) {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(t.finish_with(outcome, |_| None));
    }

    fn armed_in(home: &tempfile::TempDir) -> Telemetry {
        Telemetry::armed_for_test(home.path().to_path_buf())
    }

    #[test]
    fn armed_finish_logs_one_payload_line_then_the_skip_reason() {
        let home = tempfile::tempdir().unwrap();
        let t = armed_in(&home);
        t.start(&parsed_command(&["lock"]));
        let logs = capture_logs(|| finish_blocking(&t, &Ok(ExitCode::Success)));
        let lines: Vec<&str> = logs.lines().collect();
        assert_eq!(lines.len(), 2, "payload line + skip line expected, got: {logs}");
        assert!(lines[0].contains("grim::telemetry"), "{}", lines[0]);
        let json = lines[0].split_once("telemetry payload ").expect("payload message").1;
        let doc: serde_json::Value = serde_json::from_str(json).expect("the rest of the line is the JSON object");
        assert_eq!(doc["events"][0]["name"], "grim.command");
        assert_eq!(doc["events"][0]["attributes"]["grim.command.name"], "lock");
        assert!(
            lines[1].contains("grim::telemetry") && lines[1].contains("not sent"),
            "{}",
            lines[1]
        );
    }

    #[test]
    fn a_panic_inside_the_telemetry_task_becomes_a_debug_line() {
        let logs = capture_logs(|| {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            rt.block_on(isolated(async { panic!("telemetry bug") }));
        });
        assert!(logs.contains("telemetry task failed"), "{logs}");
    }

    #[test]
    fn a_telemetry_panic_skips_the_previous_hook_and_any_other_panic_reaches_it() {
        thread_local!(static SEEN: Cell<u32> = const { Cell::new(0) });
        let original = std::panic::take_hook();
        std::panic::set_hook(quiet(Box::new(|_| SEEN.set(SEEN.get() + 1))));
        // A current-thread runtime polls the spawned task on this thread, so
        // the thread-local counter sees exactly this test's panics.
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(isolated(async { panic!("telemetry bug") }));
        let after_telemetry = SEEN.get();
        let _ = std::panic::catch_unwind(|| panic!("grim bug"));
        let after_grim = SEEN.get();
        drop(std::panic::take_hook());
        std::panic::set_hook(original);
        assert_eq!((after_telemetry, after_grim), (0, 1));
    }

    #[test]
    fn unarmed_finish_logs_nothing() {
        let logs = capture_logs(|| finish_blocking(&Telemetry::default(), &Ok(ExitCode::Success)));
        assert!(logs.is_empty(), "{logs}");
    }

    #[test]
    fn a_failing_command_reports_its_exit_code_and_error_type() {
        let home = tempfile::tempdir().unwrap();
        let t = armed_in(&home);
        let logs = capture_logs(|| finish_blocking(&t, &Err(anyhow::anyhow!("boom"))));
        let json = logs.lines().next().unwrap().split_once("telemetry payload ").unwrap().1;
        let doc: serde_json::Value = serde_json::from_str(json).unwrap();
        assert_eq!(doc["events"][0]["attributes"]["process.exit.code"], 1);
        assert_eq!(doc["events"][0]["attributes"]["error.type"], "failure");
    }

    /// The exemption this module rests on is granted only for these exact
    /// dependency lines: a version drift or a re-enabled client feature
    /// changes what links into every `grim` binary.
    #[test]
    fn opentelemetry_dependencies_stay_pinned_and_featureless() {
        let manifest: toml::Table = include_str!("../Cargo.toml").parse().unwrap();
        let deps = manifest["dependencies"].as_table().unwrap();
        let pinned = [
            "opentelemetry",
            "opentelemetry_sdk",
            "opentelemetry-otlp",
            "opentelemetry-http",
        ];
        let forbidden = [
            "reqwest-client",
            "reqwest-blocking-client",
            "grpc-tonic",
            "internal-logs",
        ];
        for name in pinned {
            let dep = deps[name]
                .as_table()
                .unwrap_or_else(|| panic!("{name} must be a table"));
            assert_eq!(
                dep["version"].as_str(),
                Some("=0.33.1"),
                "{name} must be pinned exactly"
            );
            assert_eq!(
                dep["default-features"].as_bool(),
                Some(false),
                "{name} must opt out of defaults"
            );
            let features = dep
                .get("features")
                .and_then(|f| f.as_array())
                .cloned()
                .unwrap_or_default();
            for feature in features {
                let feature = feature.as_str().unwrap();
                assert!(!forbidden.contains(&feature), "{name} must not enable `{feature}`");
            }
        }
        for name in deps.keys().filter(|k| k.starts_with("opentelemetry")) {
            assert!(pinned.contains(&name.as_str()), "unpinned OpenTelemetry crate `{name}`");
        }
    }

    #[test]
    fn records_after_finish_are_dropped() {
        let home = tempfile::tempdir().unwrap();
        let t = armed_in(&home);
        finish_blocking(&t, &Ok(ExitCode::Success));
        t.update(|s| s.scope = Some(ConfigScope::Global));
        assert!(t.snapshot().unwrap().scope.is_none());
    }

    #[test]
    fn finish_runs_once() {
        let home = tempfile::tempdir().unwrap();
        let t = armed_in(&home);
        finish_blocking(&t, &Ok(ExitCode::Success));
        assert!(t.snapshot().unwrap().finished);
        let logs = capture_logs(|| finish_blocking(&t, &Ok(ExitCode::Success)));
        assert!(logs.is_empty(), "a second finish must deliver nothing: {logs}");
    }
}
