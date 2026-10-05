//! Explicit tmux-aware SSH session helpers.
use crate::{Session, terminal};
use anyhow::{Context, Result, ensure};
use std::{
    path::Path,
    process::{Command, Stdio},
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TmuxSession {
    pub name: String,
    pub attached_clients: u32,
}

fn validate_tmux_name(name: &str) -> Result<()> {
    ensure!(
        !name.is_empty() && name.len() <= 64,
        "tmux session name must be 1-64 bytes"
    );
    ensure!(
        !name.starts_with('-')
            && name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-')),
        "tmux session name may contain only ASCII letters, digits, '_' and '-', and may not start with '-'"
    );
    Ok(())
}

fn require_tmux_compatible_profile(session: &Session) -> Result<()> {
    session.ssh_args()?;
    ensure!(
        session.ssh.remote_command.is_empty(),
        "tmux integration requires the profile remote command to be empty"
    );
    Ok(())
}

fn tmux_session_with_command(session: &Session, command: String) -> Result<Session> {
    require_tmux_compatible_profile(session)?;
    let mut next = session.clone();
    next.ssh.remote_command = command;
    Ok(next)
}

pub fn attach_session(session: &Session, name: &str) -> Result<Session> {
    validate_tmux_name(name)?;
    tmux_session_with_command(session, format!("exec tmux attach-session -t {name}"))
}

pub fn create_session(session: &Session, name: &str) -> Result<Session> {
    validate_tmux_name(name)?;
    tmux_session_with_command(session, format!("exec tmux new-session -s {name}"))
}

fn list_command() -> &'static str {
    "command -v tmux >/dev/null 2>&1 || exit 127; tmux list-sessions -F '#{session_name}|#{session_attached}' 2>/dev/null || true"
}

pub fn list_args(session: &Session, config: Option<&Path>) -> Result<Vec<String>> {
    require_tmux_compatible_profile(session)?;
    let mut probe = session.clone();
    probe.ssh.remote_command = list_command().into();
    let mut args = terminal::launch_args(&probe, config)?;
    if let Some(index) = args.iter().position(|arg| arg == "-tt") {
        args[index] = "-T".into();
    }
    let destination = args
        .iter()
        .position(|arg| arg == "--")
        .context("internal SSH argv is missing option terminator")?;
    args.splice(
        destination..destination,
        ["-o".into(), "BatchMode=yes".into()],
    );
    Ok(args)
}

pub fn parse_sessions(output: &str) -> Result<Vec<TmuxSession>> {
    let mut sessions = Vec::new();
    for line in output.lines().filter(|line| !line.trim().is_empty()) {
        let (name, attached) = line
            .split_once('|')
            .context("unexpected tmux list-sessions output")?;
        validate_tmux_name(name)?;
        let attached_clients = attached
            .trim()
            .parse::<u32>()
            .context("invalid tmux attached-client count")?;
        sessions.push(TmuxSession {
            name: name.to_owned(),
            attached_clients,
        });
    }
    sessions.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(sessions)
}

pub fn list_sessions(session: &Session, config: Option<&Path>) -> Result<Vec<TmuxSession>> {
    terminal::check_openssh(Path::new("ssh"))?;
    let output = Command::new("ssh")
        .args(list_args(session, config)?)
        .stdin(Stdio::null())
        .output()
        .context("query remote tmux sessions through OpenSSH")?;
    if output.status.code() == Some(127) {
        anyhow::bail!("tmux is not installed or not available on the remote PATH");
    }
    ensure!(
        output.status.success(),
        "tmux session query failed: {}",
        String::from_utf8_lossy(&output.stderr).trim()
    );
    parse_sessions(&String::from_utf8_lossy(&output.stdout))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_names_are_explicit_and_commands_are_non_destructive() {
        let session = Session {
            name: "tmux".into(),
            host: "example".into(),
            ..Session::default()
        };
        assert_eq!(
            attach_session(&session, "ops_01").unwrap().ssh.remote_command,
            "exec tmux attach-session -t ops_01"
        );
        assert_eq!(
            create_session(&session, "ops-02").unwrap().ssh.remote_command,
            "exec tmux new-session -s ops-02"
        );
        for bad in ["", "-bad", "bad name", "bad;touch", "bad:window", "bad.session"] {
            assert!(attach_session(&session, bad).is_err(), "{bad:?}");
            assert!(create_session(&session, bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn parser_returns_selectable_session_state() {
        assert_eq!(
            parse_sessions("alpha|0\nbeta-2|3\n").unwrap(),
            vec![
                TmuxSession {
                    name: "alpha".into(),
                    attached_clients: 0,
                },
                TmuxSession {
                    name: "beta-2".into(),
                    attached_clients: 3,
                },
            ]
        );
    }
}
