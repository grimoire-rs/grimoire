// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! Canonical command names for the `grim.command.name` attribute.

use crate::Command;
use crate::command::config::{ConfigCommand, RegistryCommand};
use crate::command::export::ExportCommand;
use crate::command::hook::HookCommand;

/// Canonical `(name, subcommand)` for a parsed command.
///
/// The top level is an exhaustive match, so a new command is a compile error
/// until it is named. The nested trees (`config`, `export`, `hook`) are
/// exhaustive matches too; aliases report their canonical name.
pub fn command_name(command: Option<&Command>) -> (&'static str, Option<&'static str>) {
    let name = match command {
        None => return (super::UNKNOWN_COMMAND, None),
        Some(Command::Config(args)) => {
            return (
                "config",
                Some(match &args.command {
                    ConfigCommand::Get { .. } => "get",
                    ConfigCommand::Set { .. } => "set",
                    ConfigCommand::Unset { .. } => "unset",
                    ConfigCommand::List { .. } => "list",
                    ConfigCommand::Registry(registry) => match &registry.command {
                        RegistryCommand::Add { .. } => "registry add",
                        RegistryCommand::Set { .. } => "registry set",
                        RegistryCommand::Rm { .. } => "registry rm",
                        RegistryCommand::Use { .. } => "registry use",
                        RegistryCommand::Show { .. } => "registry show",
                        RegistryCommand::List => "registry list",
                        RegistryCommand::Fields => "registry fields",
                    },
                }),
            );
        }
        Some(Command::Export(args)) => {
            return (
                "export",
                Some(match &args.command {
                    ExportCommand::Plugin(_) => "plugin",
                    ExportCommand::Marketplace(_) => "marketplace",
                }),
            );
        }
        Some(Command::Hook(args)) => {
            return (
                "hook",
                Some(match &args.command {
                    HookCommand::List(_) => "list",
                    HookCommand::Allow(_) => "allow",
                    HookCommand::Revoke(_) => "revoke",
                }),
            );
        }
        Some(Command::Context(_)) => "context",
        Some(Command::Init(_)) => "init",
        Some(Command::Lock(_)) => "lock",
        Some(Command::Install(_)) => "install",
        Some(Command::Update(_)) => "update",
        Some(Command::Status(_)) => "status",
        Some(Command::Build(_)) => "build",
        Some(Command::Release(_)) => "release",
        Some(Command::Publish(_)) => "publish",
        Some(Command::Add(_)) => "add",
        Some(Command::Remove(_)) => "remove",
        Some(Command::Uninstall(_)) => "uninstall",
        Some(Command::Search(_)) => "search",
        Some(Command::Rate(_)) => "rate",
        Some(Command::Fetch(_)) => "fetch",
        Some(Command::Describe(_)) => "describe",
        Some(Command::Schema(_)) => "schema",
        Some(Command::Completions(_)) => "completions",
        Some(Command::Tui(_)) => "tui",
        Some(Command::Mcp(_)) => "mcp",
        Some(Command::Login(_)) => "login",
        Some(Command::Logout(_)) => "logout",
    };
    (name, None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_command_is_unknown() {
        assert_eq!(command_name(None), ("unknown", None));
    }

    #[test]
    fn mcp_and_tui_have_their_canonical_names() {
        // `start` keys the long-lived rule on these two exact strings.
        let parsed = crate::telemetry::parsed_command;
        assert_eq!(command_name(Some(&parsed(&["mcp"]))).0, "mcp");
        assert_eq!(command_name(Some(&parsed(&["tui"]))).0, "tui");
    }

    /// Real argv through clap, so aliases and nesting are exercised as typed.
    #[test]
    fn every_command_variant_has_its_canonical_name() {
        let table: &[(&[&str], &str, Option<&str>)] = &[
            (&["config", "get", "k"], "config", Some("get")),
            (&["config", "set", "k", "v"], "config", Some("set")),
            (&["config", "unset", "k"], "config", Some("unset")),
            (&["config", "list"], "config", Some("list")),
            (
                &["config", "registry", "add", "a", "--oci", "r"],
                "config",
                Some("registry add"),
            ),
            (&["config", "registry", "set", "a"], "config", Some("registry set")),
            (&["config", "registry", "rm", "a"], "config", Some("registry rm")),
            (&["config", "registry", "use", "a"], "config", Some("registry use")),
            (&["config", "registry", "show", "a"], "config", Some("registry show")),
            (&["config", "registry", "list"], "config", Some("registry list")),
            (&["config", "registry", "fields"], "config", Some("registry fields")),
            (&["context"], "context", None),
            (&["init"], "init", None),
            (&["lock"], "lock", None),
            (&["install"], "install", None),
            (&["update"], "update", None),
            (&["status"], "status", None),
            (&["build", "."], "build", None),
            (&["release", ".", "a/b:1"], "release", None),
            (&["publish"], "publish", None),
            (&["add", "a/b"], "add", None),
            (&["remove", "skill", "a"], "remove", None),
            (&["uninstall", "skill", "a"], "uninstall", None),
            (&["search"], "search", None),
            (&["rate", "a/b"], "rate", None),
            (&["fetch", "a/b"], "fetch", None),
            (&["describe", "a/b"], "describe", None),
            (&["export", "plugin", "a/b"], "export", Some("plugin")),
            (&["export", "marketplace"], "export", Some("marketplace")),
            (&["schema", "--kind", "config"], "schema", None),
            (&["completions", "bash"], "completions", None),
            (&["tui"], "tui", None),
            (&["mcp"], "mcp", None),
            (&["hook", "list"], "hook", Some("list")),
            (&["hook", "allow"], "hook", Some("allow")),
            (&["hook", "revoke"], "hook", Some("revoke")),
            (&["hook", "deny"], "hook", Some("revoke")),
            (&["login", "r.example"], "login", None),
            (&["logout", "r.example"], "logout", None),
        ];
        for (argv, name, sub) in table {
            let parsed = crate::telemetry::parsed_command(argv);
            assert_eq!(command_name(Some(&parsed)), (*name, *sub), "argv: {argv:?}");
        }
    }
}
