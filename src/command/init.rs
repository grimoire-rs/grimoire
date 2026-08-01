// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! `grim init` — create a fresh `grimoire.toml`.
//!
//! Project scope writes `./grimoire.toml`; `--global` writes
//! `$GRIM_HOME/grimoire.toml`. An existing file is never overwritten
//! (exit 64). When `--registry` is given (or the global `--registry` flag
//! / `$GRIM_DEFAULT_REGISTRY` is set), the body includes a `[[registries]]`
//! entry with `default = true` — the canonical on-disk shape that the
//! resolver treats as authoritative. When no registry is supplied the body
//! contains only empty `[skills]` / `[rules]` tables. The built-in fallback
//! registry is never snapshotted — it applies implicitly and must stay
//! floating.
//!
//! `init` also seeds `[options].clients` from the clients detected for the
//! scope. This is the one moment grim may write a client selection: the user
//! asked for a config file, so recording what was detected *then* is an
//! answer rather than a side effect. Nothing else auto-persists a client
//! set, and an empty detection writes no `[options]` table at all — the
//! generic-client fallback stays a runtime decision, recomputed each run.
//!
//! Seeding the key is not inert: an *explicitly set* `[options].clients` is
//! the gate for `status`'s `clients_missing`/`clients_extra` drift report
//! (`status.rs`) and for `update`'s dropped-client reaper (`update.rs`),
//! both of which stay off under autodetect. A config written by `init`
//! therefore has both active from the first run — documented under
//! `grim init` in `docs/src/commands.md`.

use anyhow::Context as _;
use clap::Args;

use crate::api::artifact_status::InitStatus;
use crate::api::init_report::InitReport;
use crate::cli::exit_code::ExitCode;
use crate::config::config_error::{ConfigError, ConfigErrorKind};
use crate::config::scope::ConfigScope;
use crate::context::Context;

/// `grim init` arguments.
#[derive(Debug, Args)]
pub struct InitArgs {
    /// Seed the default registry as a `[[registries]]` entry with
    /// `default = true`.
    #[arg(long)]
    pub registry: Option<String>,
}

/// Run `grim init`.
///
/// # Errors
///
/// Returns a [`ConfigError`] (`ConfigAlreadyExists` ⇒ exit 64, I/O ⇒ 74)
/// if the file exists or cannot be written.
pub async fn run(ctx: &Context, args: &InitArgs) -> anyhow::Result<(InitReport, ExitCode)> {
    let (path, scope) = if ctx.global() {
        (ctx.paths().global_config(), ConfigScope::Global)
    } else {
        let cwd = std::env::current_dir().context("resolving the current directory for `grim init`")?;
        (cwd.join("grimoire.toml"), ConfigScope::Project)
    };

    // Detect against the directory the config will govern, so a project init
    // reads the workspace's vendor dirs and a global init reads `$HOME`'s.
    let workspace = path.parent().unwrap_or(&path).to_path_buf();
    let detected = crate::install::target::detect_clients(&workspace, scope);

    let body = render_config(snapshot_registry(ctx, args.registry.as_deref()), &detected);
    create_config(&path, &body)?;

    let report = InitReport::new(path, scope, InitStatus::Created);
    Ok((report, ExitCode::Success))
}

/// Create `path` with `body`, refusing to overwrite an existing config.
///
/// The refusal is a check-then-act, so it is only sound under the config
/// advisory lock — acquired first, exactly like every other config writer
/// (`add`, `config`, the TUI). Unlocked, two racing inits both passed the
/// existence check and the last write won silently. The lock lives on a
/// sidecar, so it works on a config that does not exist yet.
///
/// The write goes through the shared atomic seam (temp file + rename), so
/// an interrupted init leaves either no file or a complete one — never a
/// truncated `grimoire.toml` that the existence check above would then
/// protect forever, wedging the workspace at exit 78.
///
/// # Errors
///
/// [`ConfigErrorKind::ConfigAlreadyExists`] (exit 64) when the file is
/// already there, a lock error (exit 75) when another writer holds the
/// config lock, or [`ConfigErrorKind::Io`] (74) for a failed write.
fn create_config(path: &std::path::Path, body: &str) -> anyhow::Result<()> {
    let _guard = super::grim(crate::lock::file_lock::ConfigFileLock::try_acquire(
        &super::scope_resolution::lockable_path(path),
    ))?;
    if path.exists() {
        return Err(crate::error::Error::from(ConfigError::new(path, ConfigErrorKind::ConfigAlreadyExists)).into());
    }
    crate::store::atomic_write::atomic_write(path, body.as_bytes())
        .map_err(|e| crate::error::Error::from(ConfigError::new(path, ConfigErrorKind::Io(e))))?;
    Ok(())
}

/// The registry to snapshot into the seed config: `--registry` on `init`
/// wins, then the global `--registry` flag, then `$GRIM_DEFAULT_REGISTRY`.
/// The built-in fallback registry is deliberately NOT snapshotted — pinning
/// it would freeze a default that should keep following the binary.
fn snapshot_registry<'a>(ctx: &'a Context, explicit: Option<&'a str>) -> Option<&'a str> {
    explicit.or_else(|| ctx.registry_flag()).or_else(|| ctx.registry_env())
}

/// Render the seed config. When a registry is given, emit a `[[registries]]`
/// entry with `default = true` — the canonical on-disk shape the resolver
/// treats as authoritative. When none is given, emit only the empty
/// `[skills]` / `[rules]` tables (no `[[registries]]`, no `[options]`).
///
/// `clients` seeds `[options].clients` with the detected set. An empty slice
/// emits no `[options]` table, leaving the scope on autodetect — writing
/// `clients = []` would be indistinguishable from an unset value anyway
/// (`skip_serializing_if = "Vec::is_empty"`), and writing the generic
/// fallback would persist a decision that must stay recomputed.
///
/// The locator's shape picks the key: an index-shaped value (`http(s)://`,
/// `git+…`, `ssh://`, `git@…`, `….git`) seeds `index = …`, anything else is
/// a plain OCI registry ref and seeds `oci = …` — so accepting the TUI init
/// dialog's index pre-fill persists a browse source that actually lists
/// packages (GHCR-style registries gate `_catalog`).
///
/// The locator is TOML-escaped via `toml::Value::String` to handle any
/// embedded quotes or backslashes (e.g. from unusual registry
/// configurations).
fn render_config(registry: Option<&str>, clients: &[crate::install::ClientTarget]) -> String {
    let mut out = String::new();
    // Seed the #:schema editor directive (taplo / Even Better TOML) so a
    // fresh config validates in the editor out of the box; write_config
    // preserves it across every later rewrite.
    out.push_str(&format!("#:schema {}\n\n", crate::command::schema::config_schema_id()));
    if let Some(reg) = registry {
        // TOML-escape the value so quotes or backslashes in the locator
        // produce valid TOML, consistent with how `write_config` in
        // `add.rs` escapes tree_separators.
        let escaped = toml::Value::String(reg.to_string()).to_string();
        let key = if crate::config::registry_resolve::classify_index(reg).is_some() {
            "index"
        } else {
            "oci"
        };
        out.push_str("[[registries]]\n");
        out.push_str(&format!("{key} = {escaped}\n"));
        out.push_str("default = true\n\n");
    }
    if !clients.is_empty() {
        // Client names are a closed `ClientTarget` set (never user input), so
        // the bare-string form is always valid TOML.
        let list = clients
            .iter()
            .map(|c| format!("\"{c}\""))
            .collect::<Vec<_>>()
            .join(", ");
        out.push_str(&format!("[options]\nclients = [{list}]\n\n"));
    }
    out.push_str("[skills]\n\n[rules]\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_config_refuses_an_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("grimoire.toml");
        create_config(&path, "[skills]\n").expect("first create succeeds");

        let err = create_config(&path, "[rules]\n").expect_err("second create must refuse");
        assert_eq!(crate::error::classify(&err).exit, ExitCode::UsageError);
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "[skills]\n",
            "the refused init must leave the first config untouched"
        );
    }

    #[test]
    fn create_config_refuses_while_another_writer_holds_the_config_lock() {
        // Regression: init was the one config writer that took no flock, so
        // two racing inits both passed the existence check and the last write
        // won silently. It must now contend like every other config writer.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("grimoire.toml");
        let _held = crate::lock::file_lock::ConfigFileLock::try_acquire(&path).expect("hold the config lock");

        let err = create_config(&path, "[skills]\n").expect_err("a held config lock must refuse the write");
        assert_eq!(crate::error::classify(&err).exit, ExitCode::TempFail);
        assert!(!path.exists(), "a refused init must write nothing");
    }

    #[test]
    fn render_seeds_schema_directive_first_and_parses() {
        for registry in [None, Some("ghcr.io/acme")] {
            let body = render_config(registry, &[]);
            assert!(
                body.starts_with("#:schema https://grimoire.rs/schemas/grimoire-config.schema.json\n"),
                "init must seed the #:schema directive as the first line: {body}"
            );
            crate::config::project_config::ProjectConfig::from_toml_str(&body).expect("seeded config must still parse");
        }
    }

    #[test]
    fn render_includes_registries_array_when_present() {
        // Contract (b): render_config(Some(…)) emits the [[registries]] shape,
        // NOT [options]/default_registry.
        let body = render_config(Some("ghcr.io/acme"), &[]);
        assert!(body.contains("[[registries]]"), "must contain [[registries]]");
        assert!(body.contains("oci = \"ghcr.io/acme\""), "must contain oci ref");
        assert!(body.contains("default = true"), "must contain default = true");
        assert!(
            !body.contains("default_registry ="),
            "must NOT contain legacy default_registry"
        );
        assert!(!body.contains("[options]"), "must NOT contain [options]");
        assert!(body.contains("[skills]"));
        assert!(body.contains("[rules]"));
    }

    #[test]
    fn render_output_parses_and_resolves_primary() {
        // Contract (b) round-trip: the shape init writes is the shape the
        // resolver treats as authoritative. Parse the rendered body and verify
        // primary_registry == the seeded url.
        use crate::config::registry_resolve::primary_registry;
        use crate::config::resolve_registries;
        let url = "ghcr.io/acme";
        let body = render_config(Some(url), &[]);
        let cfg =
            crate::config::project_config::ProjectConfig::from_toml_str(&body).expect("rendered config must parse");
        let set = resolve_registries(
            &[],
            &cfg.registries,
            cfg.options.default_registry.as_deref(),
            &[],
            None,
            crate::command::FALLBACK_REGISTRY,
            None,
        );
        assert_eq!(primary_registry(&set), url, "primary must equal the seeded url");
    }

    #[test]
    fn snapshot_registry_prefers_explicit_then_flag_then_env() {
        // Explicit `init --registry` wins over the context tiers.
        let tmp = tempfile::tempdir().unwrap();
        let hermetic = Context::hermetic(tmp.path().to_path_buf());
        assert_eq!(snapshot_registry(&hermetic, Some("init.example")), Some("init.example"));
        // Nothing anywhere ⇒ no snapshot (the built-in fallback stays
        // implicit, never written to disk).
        assert_eq!(snapshot_registry(&hermetic, None), None);

        // The global `--registry` flag is snapshotted when `init` has none.
        let opts = crate::cli::options::GlobalOptions {
            format: crate::cli::options::OutputFormat::Plain,
            color: crate::cli::color::ColorMode::Auto,
            progress: crate::cli::options::ProgressMode::Auto,
            offline: false,
            log_level: None,
            config: None,
            global: false,
            registry: vec!["flag.example".to_string()],
        };
        let ctx = Context::new(&opts);
        assert_eq!(snapshot_registry(&ctx, None), Some("flag.example"));
        assert_eq!(snapshot_registry(&ctx, Some("init.example")), Some("init.example"));
    }

    #[test]
    fn render_config_toml_escapes_url_with_special_chars() {
        // S1 (CWE-116): a registry url containing a backslash or quote must
        // produce valid TOML that round-trips to the same string — not break
        // the TOML parser or silently truncate the url.
        let url_with_backslash = r"example.io/org\repo";
        let url_with_quote = r#"example.io/org"repo"#;

        for url in &[url_with_backslash, url_with_quote] {
            let body = render_config(Some(url), &[]);
            let cfg = crate::config::project_config::ProjectConfig::from_toml_str(&body)
                .unwrap_or_else(|e| panic!("render_config({url:?}) produced invalid TOML: {e}"));
            assert_eq!(
                cfg.registries.len(),
                1,
                "must have exactly one [[registries]] entry for url={url:?}"
            );
            assert_eq!(
                cfg.registries[0].oci.as_deref(),
                Some(&**url),
                "locator must round-trip through TOML escaping for url={url:?}"
            );
        }
    }

    #[test]
    fn render_seeds_index_key_for_index_shaped_locator() {
        // An index-shaped locator (e.g. the public package index the TUI
        // init dialog pre-fills) must seed `index = …`, not an OCI `oci = …`
        // entry — an OCI entry pointing at an index browses empty.
        let body = render_config(Some("https://index.grimoire.rs"), &[]);
        assert!(
            body.contains("index = \"https://index.grimoire.rs\""),
            "index-shaped locator must seed the index key; got: {body}"
        );
        assert!(!body.contains("oci = "), "must not seed an oci key; got: {body}");
        let cfg = crate::config::project_config::ProjectConfig::from_toml_str(&body).expect("must parse");
        assert_eq!(cfg.registries[0].index.as_deref(), Some("https://index.grimoire.rs"));
        assert!(cfg.registries[0].default, "seeded entry must be the default");
    }

    #[test]
    fn render_seeds_detected_clients_into_options() {
        use crate::install::ClientTarget;
        let body = render_config(None, &[ClientTarget::Claude, ClientTarget::OpenCode]);
        assert!(body.contains("[options]"), "detected clients seed [options]: {body}");
        assert!(
            body.contains(r#"clients = ["claude", "opencode"]"#),
            "the detected set is written verbatim, in ClientTarget::ALL order: {body}"
        );
        let cfg = crate::config::project_config::ProjectConfig::from_toml_str(&body).expect("seed must parse");
        assert_eq!(cfg.options.clients, vec!["claude".to_string(), "opencode".to_string()]);
    }

    #[test]
    fn render_seeds_no_options_when_detection_is_empty() {
        // Per D5 the generic fallback is never persisted: an undetected
        // workspace stays on autodetect so the next run can recompute it.
        let body = render_config(None, &[]);
        assert!(!body.contains("[options]"), "empty detection writes no client set");
        assert!(!body.contains("agents"), "the generic fallback must never be persisted");
        let cfg = crate::config::project_config::ProjectConfig::from_toml_str(&body).expect("seed must parse");
        assert!(cfg.options.clients.is_empty());
    }

    #[test]
    fn render_omits_options_table_without_registry() {
        let body = render_config(None, &[]);
        assert!(!body.contains("[options]"));
        // First content after the seeded #:schema directive is [skills].
        let first_content = body.lines().find(|l| !l.trim().is_empty() && !l.starts_with('#'));
        assert_eq!(first_content, Some("[skills]"));
        assert!(body.contains("[rules]"));
        // The seed must parse back as a valid (empty) config.
        let cfg = crate::config::project_config::ProjectConfig::from_toml_str(&body).unwrap();
        assert!(cfg.set.skills.is_empty());
        assert!(cfg.set.rules.is_empty());
    }
}
