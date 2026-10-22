// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! The one place grim spawns `git` for index clones and announce pushes,
//! with the argv, environment and process rules that keep a hostile URL or
//! an inherited environment from widening what git may do.

use std::io;
use std::path::Path;
use std::process::{Output, Stdio};
use std::time::Duration;

use tokio::process::Command;

/// Transports git may use, replacing any inherited `GIT_ALLOW_PROTOCOL`.
/// Leaving `ext` out is what stops `ext::<command>` from running a program.
const ALLOWED_PROTOCOLS: &str = "file:http:https:ssh:git";

/// A transfer slower than this for [`LOW_SPEED_SECONDS`] is abandoned.
const LOW_SPEED_BYTES_PER_SECOND: &str = "1000";
const LOW_SPEED_SECONDS: &str = "30";

/// Which caller's clause set applies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GitProfile {
    /// An index clone: redirects limited to the initial request, no askpass
    /// prompt, and a stalled transfer abandoned. The caller owns the clone
    /// argv (`--no-local`, `--`).
    IndexClone,
    /// The announce push and its probes: the caller's argv unchanged.
    Announce,
}

#[derive(Debug, Clone, Copy)]
pub struct GitOpts {
    /// Kills git once elapsed; `None` waits for it.
    pub timeout: Option<Duration>,
    pub profile: GitProfile,
}

/// Runs `git` with `args` in `cwd`, stdin closed, the child killed when the
/// returned future is dropped.
///
/// # Errors
///
/// git could not be spawned, or the timeout elapsed (`ErrorKind::TimedOut`).
pub async fn git_cmd(args: &[&str], cwd: Option<&Path>, opts: GitOpts) -> io::Result<Output> {
    let mut cmd = build_command(args, cwd, opts);
    match opts.timeout {
        None => cmd.output().await,
        Some(limit) => tokio::time::timeout(limit, cmd.output())
            .await
            .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, format!("git did not finish within {limit:?}")))?,
    }
}

fn build_command(args: &[&str], cwd: Option<&Path>, opts: GitOpts) -> Command {
    let mut cmd = Command::new("git");
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }
    // A global `-c` must precede the verb. Without it a clone URL or push
    // target of the form `ext::sh -c '…'` runs an arbitrary command.
    cmd.args(["-c", "protocol.ext.allow=never"]);
    if opts.profile == GitProfile::IndexClone {
        // A user-level `http.followRedirects=true` would let an index host
        // send the request, and any credential, to another origin.
        cmd.args(["-c", "http.followRedirects=initial"]);
    }
    cmd.args(args)
        .stdin(Stdio::null())
        // Dropping the future on timeout must not leave git running.
        .kill_on_drop(true)
        // A prompt on the terminal would hang a browse.
        .env("GIT_TERMINAL_PROMPT", "0")
        // An inherited allow-list naming `ext` re-enables it and overrides
        // the `-c` above.
        .env("GIT_ALLOW_PROTOCOL", ALLOWED_PROTOCOLS);
    if opts.profile == GitProfile::IndexClone {
        // An index clone has no user to ask; an askpass helper would block
        // it instead of failing. Credential helpers and ssh-agent still work.
        cmd.env("GIT_ASKPASS", "")
            .env_remove("SSH_ASKPASS")
            .env("SSH_ASKPASS_REQUIRE", "never")
            // Bounds how long a killed git's remote helper outlives it.
            .env("GIT_HTTP_LOW_SPEED_LIMIT", LOW_SPEED_BYTES_PER_SECOND)
            .env("GIT_HTTP_LOW_SPEED_TIME", LOW_SPEED_SECONDS);
    }
    cmd
}

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;
    use std::time::Instant;

    use super::*;

    fn opts(profile: GitProfile) -> GitOpts {
        GitOpts {
            timeout: Some(Duration::from_secs(30)),
            profile,
        }
    }

    fn args_of(cmd: &Command) -> Vec<String> {
        cmd.as_std()
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }

    fn env_of(cmd: &Command, key: &str) -> Option<Option<String>> {
        cmd.as_std()
            .get_envs()
            .find(|(k, _)| *k == OsStr::new(key))
            .map(|(_, v)| v.map(|v| v.to_string_lossy().into_owned()))
    }

    #[tokio::test]
    async fn git_cmd_refuses_the_ext_transport() {
        let out = git_cmd(
            &["ls-remote", "ext::sh -c touch% /tmp/pwned"],
            None,
            opts(GitProfile::Announce),
        )
        .await
        .unwrap();
        assert!(!out.status.success());
    }

    #[test]
    fn every_profile_disables_ext_and_pins_the_allow_list() {
        for profile in [GitProfile::IndexClone, GitProfile::Announce] {
            let cmd = build_command(&["ls-remote", "--", "x"], None, opts(profile));
            let args = args_of(&cmd);
            assert_eq!(args[..2], ["-c", "protocol.ext.allow=never"], "{profile:?}");
            assert_eq!(args[args.len() - 3..], ["ls-remote", "--", "x"], "{profile:?}");
            assert_eq!(
                env_of(&cmd, "GIT_ALLOW_PROTOCOL"),
                Some(Some(ALLOWED_PROTOCOLS.to_string())),
                "an inherited allow-list must be overridden: {profile:?}"
            );
            assert_eq!(
                env_of(&cmd, "GIT_TERMINAL_PROMPT"),
                Some(Some("0".to_string())),
                "{profile:?}"
            );
        }
    }

    #[test]
    fn the_allow_list_does_not_name_ext() {
        assert!(!ALLOWED_PROTOCOLS.split(':').any(|p| p == "ext"));
    }

    #[test]
    fn an_index_clone_limits_redirects_and_closes_every_prompt() {
        let cmd = build_command(&["clone", "--", "x", "y"], None, opts(GitProfile::IndexClone));
        let args = args_of(&cmd);
        assert_eq!(
            args[..4],
            ["-c", "protocol.ext.allow=never", "-c", "http.followRedirects=initial"]
        );
        assert_eq!(env_of(&cmd, "GIT_ASKPASS"), Some(Some(String::new())));
        assert_eq!(env_of(&cmd, "SSH_ASKPASS"), Some(None), "must be removed, not set");
        assert_eq!(env_of(&cmd, "SSH_ASKPASS_REQUIRE"), Some(Some("never".to_string())));
        assert_eq!(env_of(&cmd, "GIT_HTTP_LOW_SPEED_LIMIT"), Some(Some("1000".to_string())));
        assert_eq!(env_of(&cmd, "GIT_HTTP_LOW_SPEED_TIME"), Some(Some("30".to_string())));
    }

    #[test]
    fn an_announce_command_adds_nothing_beyond_the_shared_clauses() {
        let cmd = build_command(&["push", "--", "origin", "b"], None, opts(GitProfile::Announce));
        assert_eq!(
            args_of(&cmd),
            ["-c", "protocol.ext.allow=never", "push", "--", "origin", "b"]
        );
        for key in [
            "GIT_ASKPASS",
            "SSH_ASKPASS",
            "SSH_ASKPASS_REQUIRE",
            "GIT_HTTP_LOW_SPEED_LIMIT",
        ] {
            assert_eq!(
                env_of(&cmd, key),
                None,
                "{key} would change the announce credential path"
            );
        }
    }

    #[test]
    fn the_working_directory_is_passed_through() {
        let dir = Path::new("/some/where");
        let cmd = build_command(&["status"], Some(dir), opts(GitProfile::Announce));
        assert_eq!(cmd.as_std().get_current_dir(), Some(dir));
        let cmd = build_command(&["status"], None, opts(GitProfile::Announce));
        assert_eq!(cmd.as_std().get_current_dir(), None);
    }

    /// Pids of this process's direct children whose argv names `needle`. The
    /// remote helper git forks is a grandchild, so it never matches.
    #[cfg(target_os = "linux")]
    fn children_naming(needle: &str) -> Vec<u32> {
        let me = std::process::id();
        let pids = std::fs::read_dir("/proc")
            .unwrap()
            .filter_map(|e| e.ok()?.file_name().to_str()?.parse::<u32>().ok());
        pids.filter(|pid| {
            let parent = std::fs::read_to_string(format!("/proc/{pid}/stat"))
                .ok()
                .and_then(|stat| stat.rsplit_once(')')?.1.split_whitespace().nth(1)?.parse::<u32>().ok());
            let argv = std::fs::read(format!("/proc/{pid}/cmdline")).unwrap_or_default();
            parent == Some(me) && String::from_utf8_lossy(&argv).contains(needle)
        })
        .collect()
    }

    #[tokio::test]
    async fn a_stalled_server_is_killed_at_the_timeout() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        // Accepts and then says nothing, so git waits on a response forever.
        let holder = tokio::spawn(async move {
            let mut held = Vec::new();
            while let Ok((socket, _)) = listener.accept().await {
                held.push(socket);
            }
        });
        let url = format!("http://127.0.0.1:{port}/index.git");
        let started = Instant::now();

        let run = tokio::spawn({
            let url = url.clone();
            async move {
                git_cmd(
                    &["ls-remote", "--", &url],
                    None,
                    GitOpts {
                        timeout: Some(Duration::from_secs(2)),
                        profile: GitProfile::IndexClone,
                    },
                )
                .await
            }
        });
        #[cfg(target_os = "linux")]
        {
            let seen = Instant::now();
            while children_naming(&url).is_empty() {
                assert!(seen.elapsed() < Duration::from_secs(1), "git was never spawned");
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }
        let err = run.await.unwrap().unwrap_err();

        assert_eq!(err.kind(), io::ErrorKind::TimedOut);
        assert!(started.elapsed() < Duration::from_secs(10), "{:?}", started.elapsed());
        // The server stays up until here: closing it would let git exit on its
        // own and hide a child that was never killed.
        #[cfg(target_os = "linux")]
        {
            let waited = Instant::now();
            while !children_naming(&url).is_empty() {
                assert!(waited.elapsed() < Duration::from_secs(5), "git outlived the timeout");
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }
        holder.abort();
    }
}
