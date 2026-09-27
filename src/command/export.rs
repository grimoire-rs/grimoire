// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! `grim export plugin` — package locked artifacts as a harness plugin
//! (`adr_harness_plugin_export.md`, design record C-014, C-015).
//!
//! The command layer owns only the input matrix (C-014) and the client
//! chain (C-015); everything after — manifest, lock, resolution, staging,
//! placement, lock write — is [`crate::export::stage::run`]
//! (ADR § Module placement).

use std::path::PathBuf;

use clap::{Args, Subcommand};

use crate::api::export_report::ExportReport;
use crate::cli::exit_code::ExitCode;
use crate::context::Context;
use crate::export::export_error::ExportError;
use crate::export::family::{Family, family_of};
use crate::export::marketplace::validate_plugin_name;
use crate::export::stage::{self, ExportMode, ExportOptions};
use crate::install::ClientTarget;
use crate::install::target::parse_client_list;

/// `grim export` arguments.
#[derive(Debug, Args)]
pub struct ExportArgs {
    #[command(subcommand)]
    pub command: ExportCommand,
}

/// The `export` subcommand tree.
#[derive(Debug, Subcommand)]
pub enum ExportCommand {
    /// Package artifacts as a plugin for Claude-family or Agent Plugins
    /// clients, as a directory or a zip.
    Plugin(ExportPluginArgs),
}

/// `grim export plugin` arguments (C-014).
#[derive(Debug, Args)]
pub struct ExportPluginArgs {
    /// Artifact references to export as one ad-hoc plugin (no
    /// `marketplace.toml`, no lock written). Omit to export the plugins
    /// declared in the marketplace manifest.
    pub refs: Vec<String>,

    /// Plugin name for ad-hoc refs. Required with more than one reference;
    /// defaults to the reference's name with exactly one.
    #[arg(long)]
    pub name: Option<String>,

    /// Export only this declared plugin (repeatable). Defaults to every
    /// plugin the marketplace manifest declares.
    #[arg(long = "plugin", conflicts_with = "refs")]
    pub plugins: Vec<String>,

    /// The marketplace manifest declaring the plugins. Defaults to
    /// `./marketplace.toml`.
    #[arg(long, value_name = "PATH", conflicts_with = "refs")]
    pub marketplace: Option<PathBuf>,

    /// Client(s) to export for (comma-separated, repeatable): `claude`,
    /// `droid`, `junie`, `openclaw` (Claude plugin format); `copilot`,
    /// `codex`, `cursor`, `agents` (Agent Plugins format). Defaults to the
    /// config `clients` option, then `agents`.
    #[arg(long = "client")]
    pub client: Vec<String>,

    /// Write each plugin as `<name>.<client>.zip` instead of a directory.
    #[arg(long)]
    pub zip: bool,

    /// Directory the plugins are written into (created if absent).
    #[arg(long, short = 'o', value_name = "DIR", default_value = ".")]
    pub output: PathBuf,

    /// Plugin version base (semver, no build metadata). Defaults to the
    /// declared `version`, then the reference's version annotation, then
    /// `0.0.0`; grim appends `+<content hash>`.
    #[arg(long)]
    pub version: Option<String>,

    /// Plugin description, overriding the declared `description` and the
    /// reference's description annotation. grim appends omitted members
    /// and an on-ramp sentence; the whole must fit 500 characters, so a
    /// longer text is refused.
    #[arg(long)]
    pub description: Option<String>,

    /// Replace existing outputs instead of refusing them.
    #[arg(long)]
    pub force: bool,
}

/// Run `grim export`.
///
/// # Errors
///
/// As [`run_plugin`].
pub async fn run(ctx: &Context, args: &ExportArgs) -> anyhow::Result<(ExportReport, ExitCode)> {
    match &args.command {
        ExportCommand::Plugin(plugin_args) => run_plugin(ctx, plugin_args).await,
    }
}

/// Run `grim export plugin` (C-014): check the input matrix (the 64 rows
/// clap's conflicts do not cover), pick the clients ([`select_clients`]),
/// build the fetch scope and access seam, and hand the rest to
/// [`crate::export::stage::run`].
///
/// # Errors
///
/// Matrix violations (64), manifest / version / rename / empty-plugin /
/// output-exists failures (65), a client without a plugin format or a
/// member conflict (78), an unknown plugin or include (79), lock
/// contention (75), and every resolver, access and staging failure with
/// its existing classification.
pub async fn run_plugin(ctx: &Context, args: &ExportPluginArgs) -> anyhow::Result<(ExportReport, ExitCode)> {
    let mode = match (args.refs.as_slice(), &args.name) {
        ([], Some(_)) => return Err(usage("--name applies only to an ad-hoc export (positional references)")),
        ([], None) => ExportMode::Declared {
            manifest: args
                .marketplace
                .clone()
                .unwrap_or_else(|| PathBuf::from("marketplace.toml")),
            plugins: args.plugins.clone(),
        },
        ([_, _, ..], None) => return Err(usage("--name is required when exporting more than one reference")),
        (refs, name) => {
            if let Some(name) = name {
                validate_plugin_name(name).map_err(|reason| usage(format!("invalid --name '{name}': {reason}")))?;
            }
            ExportMode::AdHoc {
                refs: refs.iter().map(|r| cli_ref(r)).collect(),
                name: name.clone(),
            }
        }
    };
    let configured = if args.client.is_empty() {
        configured_clients(ctx)?
    } else {
        Vec::new()
    };
    let clients = select_clients(&args.client, &configured)?;

    // Decision 35: a declared manifest's registry context and insecure-host
    // set come from M's directory, not from the cwd project.
    let (scope, access) = match &mode {
        ExportMode::Declared { manifest, .. } => {
            let anchor = std::path::absolute(manifest)
                .ok()
                .and_then(|m| m.parent().map(std::path::Path::to_path_buf))
                .unwrap_or_else(|| PathBuf::from("."));
            (
                super::resolve_fetch_scope(ctx, false, None, Some(&anchor))?,
                super::access_seam_scoped(ctx, false, None, Some(&anchor))?,
            )
        }
        ExportMode::AdHoc { .. } => (
            super::resolve_fetch_scope(ctx, ctx.global(), ctx.config(), None)?,
            super::access_seam(ctx)?,
        ),
    };
    let progress = crate::cli::progress::select_progress(ctx.progress(), true);
    let opts = ExportOptions {
        clients: &clients,
        output_dir: &args.output,
        zip: args.zip,
        force: args.force,
        version: args.version.as_deref(),
        description: args.description.as_deref(),
        progress: progress.as_ref(),
    };
    let report = super::grim(stage::run(&mode, &opts, &scope, &access, ctx.offline()).await)?;
    Ok((report, ExitCode::Success))
}

/// `[options].clients` of the resolved project scope, or of the global
/// config when no scope resolves (C-015).
fn configured_clients(ctx: &Context) -> anyhow::Result<Vec<String>> {
    match super::scope_resolution::resolve(ctx, ctx.global(), ctx.config()) {
        Ok(scope) => Ok(scope.options.clients),
        Err(e) if super::scope_resolution::config_not_found(&e) => Ok(super::global_fallback(ctx)?.1.clients),
        Err(e) => Err(anyhow::Error::from(crate::error::Error::from(e))),
    }
}

fn usage(message: impl Into<String>) -> anyhow::Error {
    anyhow::Error::from(crate::error::Error::from(ExportError::Usage(message.into())))
}

/// The export client chain (C-015), parsing through the shared
/// `target::parse_client_list`: `explicit` (`--client`) when given —
/// an unknown name is `UnsupportedClient` 78, a client without a plugin
/// format `NoPluginFormat` 78; else `configured` (`[options].clients` of
/// the project scope, or the global config when no scope resolves), each
/// client without a format dropped with a stderr note; else `[agents]`.
/// Order is first mention, duplicates removed.
///
/// # Errors
///
/// `Usage` (64) for an explicit list naming no client (`--client ""`);
/// `UnsupportedClient` / `NoPluginFormat` (78) for an explicit client.
pub(crate) fn select_clients(
    explicit: &[String],
    configured: &[String],
) -> anyhow::Result<Vec<(ClientTarget, Family)>> {
    if !explicit.is_empty() {
        let clients = super::grim(parse_client_list(explicit))?;
        if clients.is_empty() {
            return Err(usage("--client names no client"));
        }
        return super::grim(
            clients
                .into_iter()
                .map(|client| {
                    family_of(client)
                        .map(|family| (client, family))
                        .ok_or(ExportError::NoPluginFormat { client })
                })
                .collect::<Result<Vec<_>, _>>(),
        );
    }
    let mut picked = Vec::new();
    for client in super::grim(parse_client_list(configured))? {
        match family_of(client) {
            Some(family) => picked.push((client, family)),
            None => tracing::warn!("client '{client}' has no plugin format; skipped"),
        }
    }
    if picked.is_empty() {
        picked.push((ClientTarget::Agents, Family::AgentPlugins));
    }
    Ok(picked)
}

/// An ad-hoc ref as `grim add` reads its reference (decision 41): the
/// `normalize_cli_path` form when that is a path, the raw value otherwise.
/// Before name derivation and hashing; manifest includes stay strict.
fn cli_ref(raw: &str) -> String {
    let normalized = crate::config::path_source::normalize_cli_path(raw);
    if crate::config::is_path_value(&normalized) {
        normalized.into_owned()
    } else {
        raw.to_string()
    }
}

#[cfg(test)]
mod tests {
    //! Specification tests written from the design record (C-002, C-014,
    //! C-015), not from the implementation.

    use std::path::Path;

    use clap::Parser;

    use super::*;
    use crate::error::{Error, classify_error};
    use crate::export::export_error::ExportError;
    use crate::oci::access::memory_registry::MemoryRegistry;

    const HELLO: &str = "localhost:5000/team/hello:1.0";
    const WORLD: &str = "localhost:5000/team/world:1.0";

    fn strings(values: &[&str]) -> Vec<String> {
        values.iter().map(|v| (*v).to_string()).collect()
    }

    /// Explicit `--client agents` keeps the client chain out of every
    /// matrix test; `-o` is a fresh temp dir.
    fn args(
        refs: &[&str],
        name: Option<&str>,
        plugins: &[&str],
        marketplace: Option<&Path>,
        out: &Path,
    ) -> ExportPluginArgs {
        ExportPluginArgs {
            refs: strings(refs),
            name: name.map(str::to_string),
            plugins: strings(plugins),
            marketplace: marketplace.map(Path::to_path_buf),
            client: strings(&["agents"]),
            zip: false,
            output: out.to_path_buf(),
            version: None,
            description: None,
            force: false,
        }
    }

    /// Run against an empty in-memory registry: no ad-hoc ref resolves, so a
    /// row the matrix accepts surfaces as `IncludeNotFound` naming the plugin.
    async fn run(a: &ExportPluginArgs) -> anyhow::Error {
        let home = tempfile::tempdir().unwrap();
        let ctx = Context::with_access(home.path().to_path_buf(), MemoryRegistry::new());
        run_plugin(&ctx, a)
            .await
            .expect_err("no row succeeds against an empty registry")
    }

    fn export_error(err: &anyhow::Error) -> &ExportError {
        err.chain()
            .find_map(|c| match c.downcast_ref::<Error>() {
                Some(Error::Export(e)) => Some(e),
                _ => None,
            })
            .unwrap_or_else(|| panic!("expected an export error, got {err:?}"))
    }

    fn include_not_found_plugin(err: &anyhow::Error) -> String {
        match export_error(err) {
            ExportError::IncludeNotFound { plugin, .. } => plugin.clone(),
            other => panic!("expected the row to reach resolution, got {other:?}"),
        }
    }

    // ── C-014 input matrix ────────────────────────────────────────

    #[tokio::test]
    async fn c014_one_ref_without_name_uses_the_refs_binding() {
        let out = tempfile::tempdir().unwrap();
        let err = run(&args(&[HELLO], None, &[], None, out.path())).await;
        assert_eq!(include_not_found_plugin(&err), "hello");
        assert_eq!(classify_error(&err), ExitCode::NotFound);
    }

    #[tokio::test]
    async fn c014_one_ref_with_name_uses_the_name() {
        let out = tempfile::tempdir().unwrap();
        let err = run(&args(&[HELLO], Some("team-stack"), &[], None, out.path())).await;
        assert_eq!(include_not_found_plugin(&err), "team-stack");
    }

    #[tokio::test]
    async fn c014_two_refs_with_name_is_one_merged_plugin() {
        let out = tempfile::tempdir().unwrap();
        let err = run(&args(&[HELLO, WORLD], Some("team-stack"), &[], None, out.path())).await;
        assert_eq!(include_not_found_plugin(&err), "team-stack");
    }

    #[tokio::test]
    async fn c014_two_refs_without_name_is_usage_64() {
        let out = tempfile::tempdir().unwrap();
        let err = run(&args(&[HELLO, WORLD], None, &[], None, out.path())).await;
        assert_eq!(classify_error(&err), ExitCode::UsageError);
        assert!(
            format!("{err:#}").contains("--name is required when exporting more than one reference"),
            "{err:#}"
        );
        assert!(
            std::fs::read_dir(out.path()).unwrap().next().is_none(),
            "nothing written"
        );
    }

    #[tokio::test]
    async fn c014_name_without_refs_is_usage_64() {
        let out = tempfile::tempdir().unwrap();
        let err = run(&args(&[], Some("team-stack"), &[], None, out.path())).await;
        assert_eq!(classify_error(&err), ExitCode::UsageError);
        let err = run(&args(&[], Some("team-stack"), &["team"], None, out.path())).await;
        assert_eq!(classify_error(&err), ExitCode::UsageError, "with --plugin too");
    }

    #[tokio::test]
    async fn c002_invalid_name_flag_is_usage_64() {
        let out = tempfile::tempdir().unwrap();
        for bad in ["A", "a--b", "a/b"] {
            let err = run(&args(&[HELLO], Some(bad), &[], None, out.path())).await;
            assert_eq!(classify_error(&err), ExitCode::UsageError, "{bad}");
        }
    }

    fn manifest(dir: &Path, body: &str) -> std::path::PathBuf {
        let path = dir.join("marketplace.toml");
        std::fs::write(&path, body).unwrap();
        path
    }

    #[tokio::test]
    async fn c014_unknown_declared_plugin_is_not_found_79() {
        let out = tempfile::tempdir().unwrap();
        let m = manifest(out.path(), &format!("[plugins.team]\ninclude = [\"{HELLO}\"]\n"));
        let err = run(&args(&[], None, &["ghost"], Some(&m), out.path())).await;
        assert!(
            matches!(export_error(&err), ExportError::PluginNotFound { name } if name == "ghost"),
            "{err:?}"
        );
        assert_eq!(classify_error(&err), ExitCode::NotFound);
    }

    #[tokio::test]
    async fn c014_no_declared_plugin_is_none_declared_65() {
        let out = tempfile::tempdir().unwrap();
        let m = manifest(out.path(), "");
        let err = run(&args(&[], None, &[], Some(&m), out.path())).await;
        assert!(
            matches!(export_error(&err), ExportError::NoneDeclared { .. }),
            "{err:?}"
        );
        assert_eq!(classify_error(&err), ExitCode::DataError);
        let text = format!("{err:#}");
        // `--plugin` cannot help when nothing is declared; only `<ref>… --name` can.
        assert!(text.contains("--name") && !text.contains("--plugin"), "hint: {text}");
    }

    #[tokio::test]
    async fn s008_missing_manifest_is_65_with_a_hint() {
        let out = tempfile::tempdir().unwrap();
        let m = out.path().join("marketplace.toml");
        let err = run(&args(&[], None, &[], Some(&m), out.path())).await;
        assert_eq!(classify_error(&err), ExitCode::DataError);
        let text = format!("{err:#}");
        assert!(
            text.contains("manifest not found; pass <ref>… for an ad-hoc export, or --marketplace <PATH>"),
            "{text}"
        );
    }

    /// Wraps the subcommand tree so argv parses in isolation.
    #[derive(Parser)]
    struct Harness {
        #[command(flatten)]
        export: ExportArgs,
    }

    fn parse(argv: &[&str]) -> Result<ExportPluginArgs, clap::Error> {
        let mut full = vec!["grim", "plugin"];
        full.extend_from_slice(argv);
        Harness::try_parse_from(full).map(|h| match h.export.command {
            ExportCommand::Plugin(a) => a,
        })
    }

    #[test]
    fn c014_refs_conflict_with_plugin_and_marketplace_in_clap() {
        for argv in [
            &[HELLO, "--plugin", "team"][..],
            &[HELLO, "--marketplace", "m.toml"][..],
        ] {
            let err = parse(argv).expect_err("clap conflict");
            assert_eq!(err.kind(), clap::error::ErrorKind::ArgumentConflict, "{argv:?}");
            assert_eq!(err.exit_code(), 2, "clap's own code; main maps usage errors to 64");
        }
        let ok = parse(&["--plugin", "a", "--plugin", "b", "--client", "claude,codex"]).unwrap();
        assert_eq!(ok.plugins, strings(&["a", "b"]));
        assert_eq!(ok.output, std::path::PathBuf::from("."));
    }

    #[test]
    fn c014_ad_hoc_path_refs_take_grim_adds_cli_normalization() {
        // Decision 41: as `normalize_cli_path` — OS-native separators become
        // `/` on Windows only; a registry ref is never rewritten.
        if cfg!(windows) {
            assert_eq!(cli_ref(r".\skills\x"), "./skills/x");
            assert_eq!(cli_ref(r"C:\skills\x"), "C:/skills/x");
        } else {
            assert_eq!(cli_ref(r".\skills\x"), r".\skills\x");
            assert_eq!(cli_ref(r"C:\skills\x"), r"C:\skills\x");
        }
        assert_eq!(cli_ref("./skills/x"), "./skills/x");
        assert_eq!(cli_ref(HELLO), HELLO);
    }

    // ── C-015 client chain ────────────────────────────────────────

    fn picked(explicit: &[&str], configured: &[&str]) -> Vec<(ClientTarget, Family)> {
        select_clients(&strings(explicit), &strings(configured)).unwrap()
    }

    #[test]
    fn c015_explicit_list_wins_deduped_in_first_mention_order() {
        assert_eq!(
            picked(&["codex, claude", "codex", "droid"], &["junie"]),
            vec![
                (ClientTarget::Codex, Family::AgentPlugins),
                (ClientTarget::Claude, Family::Claude),
                (ClientTarget::Droid, Family::Claude),
            ]
        );
    }

    #[test]
    fn c015_explicit_client_without_plugin_format_is_78() {
        let err = select_clients(&strings(&["claude,amp"]), &[]).unwrap_err();
        assert!(
            matches!(export_error(&err), ExportError::NoPluginFormat { client } if *client == ClientTarget::Amp),
            "{err:?}"
        );
        assert_eq!(classify_error(&err), ExitCode::ConfigError);
        let text = format!("{err:#}");
        for client in [
            "claude", "droid", "junie", "openclaw", "copilot", "codex", "cursor", "agents",
        ] {
            assert!(text.contains(client), "hint lists '{client}': {text}");
        }
        assert!(!text.contains("opencode"), "no format, not listed: {text}");
    }

    #[test]
    fn c015_explicit_list_naming_no_client_is_usage_64() {
        // An explicit `--client` never falls back: a blank list would export
        // nothing and exit 0.
        for blank in [&[""][..], &[","], &[" , ", ""]] {
            let err = select_clients(&strings(blank), &strings(&["claude"])).unwrap_err();
            assert_eq!(classify_error(&err), ExitCode::UsageError, "{blank:?}");
            assert!(format!("{err:#}").contains("--client"), "{err:#}");
        }
    }

    #[test]
    fn c015_explicit_unknown_client_is_78() {
        let err = select_clients(&strings(&["nope"]), &[]).unwrap_err();
        assert_eq!(classify_error(&err), ExitCode::ConfigError);
    }

    #[test]
    fn c015_configured_clients_without_format_are_dropped() {
        assert_eq!(
            picked(&[], &["amp", "junie,opencode", "junie"]),
            vec![(ClientTarget::Junie, Family::Claude)]
        );
    }

    #[test]
    fn c015_configured_all_without_format_falls_back_to_agents() {
        assert_eq!(
            picked(&[], &["amp", "opencode"]),
            vec![(ClientTarget::Agents, Family::AgentPlugins)]
        );
    }

    #[test]
    fn c015_nothing_configured_is_agents() {
        assert_eq!(picked(&[], &[]), vec![(ClientTarget::Agents, Family::AgentPlugins)]);
    }
}
