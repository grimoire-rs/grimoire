// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! The remedy a version-claim refusal names, per command.
//!
//! The graph that refuses holds no command, so the seam that ran it fills in
//! the `fix`: what to declare, and what to run instead.

use std::path::PathBuf;

use super::resolve_error::{ResolveError, ResolveErrorKind, Unchosen};
use crate::config::scope::{Invocation, shell_word};

/// The command a resolve ran under, which decides what its refusal tells the
/// user to run instead.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemedyCommand {
    /// `grim lock`.
    Lock,
    /// `grim add <target> <flags…>`: a rerun repeats every flag the run stands for.
    AddFlagged { target: String, flags: Vec<String> },
    /// `grim update [names…]`; no names is a bare update.
    Update { names: Vec<String> },
    /// `grim update --marketplace <manifest> <plugin>`.
    Marketplace { manifest: PathBuf, plugin: String },
}

/// Fill the `fix` of an `UnchosenRequirement` or `MajorDowngrade` refusal
/// for `command`, naming `err.grim`. Every other error returns unchanged.
/// The kind's `Display` escapes the whole fix, so text goes in raw here.
pub fn with_remedy(mut err: ResolveError, command: &RemedyCommand) -> ResolveError {
    let (declare, to, conflict, fix) = match &mut err.kind {
        ResolveErrorKind::UnchosenRequirement {
            declare, fix, shape, ..
        } => (declare.as_str(), None, matches!(shape, Unchosen::Conflict { .. }), fix),
        ResolveErrorKind::MajorDowngrade { declare, to, fix, .. } => (declare.as_str(), Some(to.as_str()), false, fix),
        _ => return err,
    };
    let grim = &err.grim;
    let name = &err.reference.name;
    let keep = format!("{} {name} as `{declare}`", err.reference.kind);
    // Only a major downgrade has a flag to offer, and every command without it
    // meets the same downgrade guard, so its fix offers no unflagged rerun.
    let flagged = |clause: &dyn Fn(&str) -> String| to.map(clause).unwrap_or_default();
    *fix = match command {
        RemedyCommand::Lock => match to {
            None => format!(
                "declare {keep} in grimoire.toml, or run `{grim} update` to resolve every pin afresh, running \
                 `{grim} update --allow-downgrade` instead if it refuses a major downgrade"
            ),
            Some(to) => {
                format!("declare {keep} in grimoire.toml, or rerun `{grim} lock --allow-downgrade` to take {to}")
            }
        },
        RemedyCommand::AddFlagged { target, flags } => add_fix(grim, &keep, to, target, flags),
        RemedyCommand::Update { names } if names.is_empty() => format!(
            "declare {keep} in grimoire.toml{}",
            flagged(&|to| format!(", or rerun `{grim} update --allow-downgrade` to take {to}"))
        ),
        RemedyCommand::Update { names } => {
            let named = names.contains(name);
            let words = names
                .iter()
                .chain((!named).then_some(name))
                .map(|n| shell_word(n))
                .collect::<Vec<_>>()
                .join(" ");
            // `--` keeps a name with a leading dash from reading as a flag.
            let instead = match to {
                // Naming the node still reads the held version that claimed the other
                // side of a conflict, so that run refuses again.
                None if named || conflict => String::new(),
                None => format!(
                    ", or name {name} too: `{grim} update -- {words}`, running `{grim} update --allow-downgrade -- \
                     {words}` instead if it refuses a major downgrade"
                ),
                Some(_) => format!(", or run `{grim} update --allow-downgrade -- {words}`"),
            };
            format!("declare {keep} in grimoire.toml{instead}")
        }
        RemedyCommand::Marketplace { manifest, plugin } => {
            let update = format!(
                "{grim} update --marketplace {} {plugin}",
                shell_word(&manifest.to_string_lossy())
            );
            let (flag, instead) = match to {
                None => (
                    "",
                    format!(", running `{update} --allow-downgrade` instead if it refuses a major downgrade"),
                ),
                Some(_) => (" --allow-downgrade", String::new()),
            };
            format!(
                "include {keep} in plugin '{plugin}' of {}, or run `{update}{flag}`, which moves every pin of the \
                 plugin whose tag moved{instead}",
                manifest.display()
            )
        }
    };
    err
}

/// The fix under `grim add <target> <flags…>`: every alternative ends in a rerun
/// of that same add.
fn add_fix(grim: &Invocation, keep: &str, to: Option<&str>, target: &str, flags: &[String]) -> String {
    let words = std::iter::once(target)
        .chain(flags.iter().map(String::as_str))
        .map(shell_word)
        .collect::<Vec<_>>()
        .join(" ");
    let add = format!("{grim} add {words}");
    let instead = match to {
        None => format!(
            "run `{grim} update`, then rerun `{add}`, running `{grim} update --allow-downgrade` instead if the \
             update refuses a major downgrade"
        ),
        Some(_) => format!("rerun `{add} --allow-downgrade`"),
    };
    format!("declare {keep} in grimoire.toml, then rerun `{add}`, or {instead}")
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::oci::reference::ArtifactRef;
    use crate::oci::{ArtifactKind, Identifier};

    fn config() -> Invocation {
        Invocation::config(Path::new("/srv/team-a/grimoire.toml"))
    }

    /// A refusal about `skill r` from a run under `grim`.
    fn refusal(kind: ResolveErrorKind, grim: &Invocation) -> ResolveError {
        let id = Identifier::parse("ghcr.io/acme/r:2").unwrap();
        ResolveError::new(ArtifactRef::registry(ArtifactKind::Skill, "r", id), kind).naming(grim)
    }

    fn unchosen() -> ResolveErrorKind {
        ResolveErrorKind::UnchosenRequirement {
            parent: "skill/q".to_string(),
            dropped: "1.6.0".to_string(),
            claim: "1.5".to_string(),
            shape: Unchosen::Move {
                from: "2.1.0".to_string(),
                to: "2.4.0".to_string(),
            },
            declare: "ghcr.io/acme/r:2.1.0".to_string(),
            fix: String::new(),
        }
    }

    fn conflict() -> ResolveErrorKind {
        ResolveErrorKind::UnchosenRequirement {
            parent: "skill/q".to_string(),
            dropped: "1.6.0".to_string(),
            claim: "1.5".to_string(),
            shape: Unchosen::Conflict {
                claims: "^1.5, ^2".to_string(),
            },
            declare: "ghcr.io/acme/r:2".to_string(),
            fix: String::new(),
        }
    }

    fn downgrade() -> ResolveErrorKind {
        ResolveErrorKind::MajorDowngrade {
            from: "3.1.0".to_string(),
            to: "2.6.0".to_string(),
            requesters: "skill/a".to_string(),
            claim: "2".to_string(),
            declare: "ghcr.io/acme/r:3.1.0".to_string(),
            fix: String::new(),
        }
    }

    fn fix(kind: ResolveErrorKind, grim: &Invocation, command: &RemedyCommand) -> String {
        match with_remedy(refusal(kind, grim), command).kind {
            ResolveErrorKind::UnchosenRequirement { fix, .. } | ResolveErrorKind::MajorDowngrade { fix, .. } => fix,
            other => panic!("not a claim refusal: {other:?}"),
        }
    }

    #[test]
    fn the_lock_remedy_names_declare_update_and_the_flag() {
        let lock = RemedyCommand::Lock;

        assert_eq!(
            fix(unchosen(), &Invocation::WALKED, &lock),
            "declare skill r as `ghcr.io/acme/r:2.1.0` in grimoire.toml, or run `grim update` to resolve every \
             pin afresh, running `grim update --allow-downgrade` instead if it refuses a major downgrade"
        );
        // An update meets the same downgrade guard, so only the flag takes the move.
        assert_eq!(
            fix(downgrade(), &Invocation::GLOBAL, &lock),
            "declare skill r as `ghcr.io/acme/r:3.1.0` in grimoire.toml, or rerun `grim --global lock \
             --allow-downgrade` to take 2.6.0"
        );
        assert_eq!(
            fix(downgrade(), &config(), &lock),
            "declare skill r as `ghcr.io/acme/r:3.1.0` in grimoire.toml, or rerun `grim --config \
             /srv/team-a/grimoire.toml lock --allow-downgrade` to take 2.6.0"
        );
    }

    #[test]
    fn the_add_remedy_ends_with_rerun_add() {
        let add = RemedyCommand::AddFlagged {
            target: "ghcr.io/acme/q:1.6".to_string(),
            flags: Vec::new(),
        };

        assert_eq!(
            fix(unchosen(), &config(), &add),
            "declare skill r as `ghcr.io/acme/r:2.1.0` in grimoire.toml, then rerun `grim --config \
             /srv/team-a/grimoire.toml add ghcr.io/acme/q:1.6`, or run `grim --config /srv/team-a/grimoire.toml \
             update`, then rerun `grim --config /srv/team-a/grimoire.toml add ghcr.io/acme/q:1.6`, running `grim \
             --config /srv/team-a/grimoire.toml update --allow-downgrade` instead if the update refuses a major \
             downgrade"
        );
        assert_eq!(
            fix(downgrade(), &Invocation::WALKED, &add),
            "declare skill r as `ghcr.io/acme/r:3.1.0` in grimoire.toml, then rerun `grim add ghcr.io/acme/q:1.6`, \
             or rerun `grim add ghcr.io/acme/q:1.6 --allow-downgrade`"
        );
    }

    #[test]
    fn the_update_remedy_names_the_node_too() {
        let update = RemedyCommand::Update {
            names: vec!["a".to_string(), "b".to_string()],
        };

        assert_eq!(
            fix(unchosen(), &Invocation::GLOBAL, &update),
            "declare skill r as `ghcr.io/acme/r:2.1.0` in grimoire.toml, or name r too: `grim --global update -- \
             a b r`, running `grim --global update --allow-downgrade -- a b r` instead if it refuses a major \
             downgrade"
        );
        // Naming r leaves the held version that claimed the other side in the
        // graph, so the conflict form offers only the declaration.
        assert_eq!(
            fix(conflict(), &Invocation::GLOBAL, &update),
            "declare skill r as `ghcr.io/acme/r:2` in grimoire.toml"
        );
        // Naming r without the flag meets the same downgrade guard.
        assert_eq!(
            fix(downgrade(), &Invocation::WALKED, &update),
            "declare skill r as `ghcr.io/acme/r:3.1.0` in grimoire.toml, or run `grim update --allow-downgrade -- \
             a b r`"
        );

        // A node the run already named needs no second mention.
        let named = RemedyCommand::Update {
            names: vec!["a".to_string(), "r".to_string()],
        };
        assert_eq!(
            fix(downgrade(), &Invocation::WALKED, &named),
            "declare skill r as `ghcr.io/acme/r:3.1.0` in grimoire.toml, or run `grim update --allow-downgrade -- \
             a r`"
        );
    }

    #[test]
    fn the_bare_update_remedy_never_says_name_it_too() {
        let update = RemedyCommand::Update { names: Vec::new() };

        let unchosen_fix = fix(unchosen(), &Invocation::WALKED, &update);
        assert_eq!(
            unchosen_fix,
            "declare skill r as `ghcr.io/acme/r:2.1.0` in grimoire.toml"
        );
        let downgrade_fix = fix(downgrade(), &config(), &update);
        assert_eq!(
            downgrade_fix,
            "declare skill r as `ghcr.io/acme/r:3.1.0` in grimoire.toml, or rerun `grim --config \
             /srv/team-a/grimoire.toml update --allow-downgrade` to take 2.6.0"
        );
        for text in [unchosen_fix, downgrade_fix] {
            assert!(!text.contains("too"), "{text}");
        }
    }

    #[test]
    fn the_marketplace_remedy_names_update_marketplace() {
        let marketplace = RemedyCommand::Marketplace {
            manifest: PathBuf::from("/w/my market/marketplace.toml"),
            plugin: "team".to_string(),
        };

        assert_eq!(
            fix(unchosen(), &Invocation::WALKED, &marketplace),
            "include skill r as `ghcr.io/acme/r:2.1.0` in plugin 'team' of /w/my market/marketplace.toml, or run \
             `grim update --marketplace '/w/my market/marketplace.toml' team`, which moves every pin of the plugin \
             whose tag moved, running `grim update --marketplace '/w/my market/marketplace.toml' team \
             --allow-downgrade` instead if it refuses a major downgrade"
        );
        assert_eq!(
            fix(downgrade(), &Invocation::GLOBAL, &marketplace),
            "include skill r as `ghcr.io/acme/r:3.1.0` in plugin 'team' of /w/my market/marketplace.toml, or run \
             `grim --global update --marketplace '/w/my market/marketplace.toml' team --allow-downgrade`, which \
             moves every pin of the plugin whose tag moved"
        );
    }

    #[test]
    fn with_remedy_leaves_other_errors_unchanged() {
        let conflict = || ResolveErrorKind::BundleConflict {
            kind: ArtifactKind::Skill,
            name: "r".to_string(),
            sources: "acme/r:1, acme/r:2".to_string(),
            others: String::new(),
            locations: vec!["ghcr.io/acme/r".to_string()],
            from_bundles: false,
        };
        let commands = [
            RemedyCommand::Lock,
            RemedyCommand::AddFlagged {
                target: "ghcr.io/acme/q:1.6".to_string(),
                flags: Vec::new(),
            },
            RemedyCommand::Update {
                names: vec!["a".to_string()],
            },
            RemedyCommand::Marketplace {
                manifest: PathBuf::from("/w/marketplace.toml"),
                plugin: "team".to_string(),
            },
        ];

        let kinds: [fn() -> ResolveErrorKind; 2] = [|| ResolveErrorKind::TagNotFound, conflict];
        for command in &commands {
            for kind in kinds {
                let before = refusal(kind(), &config()).to_string();
                let after = with_remedy(refusal(kind(), &config()), command);
                assert_eq!(after.to_string(), before, "{command:?}");
                assert_eq!(after.grim, config());
            }
        }
    }
}
