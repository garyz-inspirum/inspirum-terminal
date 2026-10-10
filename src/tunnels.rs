//! Centralised SSH tunnel manager model.
//!
//! One forwarding-only OpenSSH process per saved profile runs that profile's
//! `-L`/`-R`/`-D` forwards (see [`crate::terminal::start_tunnels`]). This
//! module owns the GUI-facing state: a flat list of every forward across every
//! saved profile, the per-profile process, and the status each row shows.
//! It deliberately contains no Iced types so it can be unit tested.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Result, ensure};

use crate::terminal::TunnelProcess;
use crate::{Session, session_profile_key, session_requires_forward_risk_ack};

/// Which OpenSSH forwarding flag a row came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ForwardKind {
    Local,
    Remote,
    Dynamic,
}

impl ForwardKind {
    pub fn short(self) -> &'static str {
        match self {
            Self::Local => "L",
            Self::Remote => "R",
            Self::Dynamic => "D",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Local => "Local",
            Self::Remote => "Remote",
            Self::Dynamic => "Dynamic (SOCKS)",
        }
    }
}

/// One line in the manager: a single forward of a single saved profile.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ForwardRow {
    pub profile_key: String,
    pub profile_name: String,
    pub kind: ForwardKind,
    /// `bind:port` (or `port`) the listener side of the forward.
    pub listen: String,
    /// `host:port` on the far side; empty for dynamic forwards.
    pub target: String,
    pub spec: String,
}

/// Split an OpenSSH forward spec into its listener and target halves for
/// display. Specs are already validated by `SshOptions`; this only formats.
pub fn split_spec(kind: ForwardKind, spec: &str) -> (String, String) {
    let spec = spec.trim();
    if kind == ForwardKind::Dynamic {
        return (spec.to_owned(), String::new());
    }
    // Forms: port:host:hostport, bind:port:host:hostport, with optional
    // [v6]:port brackets. Walk from the right: hostport, host, then listener.
    let (left, hostport) = match spec.rsplit_once(':') {
        Some(split) => split,
        None => return (spec.to_owned(), String::new()),
    };
    let (listen, host) = split_host_from_right(left);
    (listen.to_owned(), format!("{host}:{hostport}"))
}

fn split_host_from_right(value: &str) -> (&str, &str) {
    if value.ends_with(']')
        && let Some(open) = value.rfind('[')
        && open > 0
        && value.as_bytes()[open - 1] == b':'
    {
        return (&value[..open - 1], &value[open..]);
    }
    match value.rsplit_once(':') {
        Some((listen, host)) => (listen, host),
        None => ("", value),
    }
}

pub fn rows_for(session: &Session) -> Vec<ForwardRow> {
    let key = session_profile_key(session);
    let mut rows = Vec::new();
    for (kind, specs) in [
        (ForwardKind::Local, &session.ssh.local_forwards),
        (ForwardKind::Remote, &session.ssh.remote_forwards),
        (ForwardKind::Dynamic, &session.ssh.dynamic_forwards),
    ] {
        for spec in specs {
            let (listen, target) = split_spec(kind, spec);
            rows.push(ForwardRow {
                profile_key: key.clone(),
                profile_name: session.name.clone(),
                kind,
                listen,
                target,
                spec: spec.clone(),
            });
        }
    }
    rows
}

/// Every forward of every saved profile, in profile order.
pub fn all_rows(profiles: &[Session]) -> Vec<ForwardRow> {
    profiles.iter().flat_map(rows_for).collect()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    Stopped,
    Running { pid: u32 },
    Failed(String),
}

impl Status {
    pub fn label(&self) -> String {
        match self {
            Self::Stopped => "Stopped".into(),
            Self::Running { pid } => format!("Running · ssh {pid}"),
            Self::Failed(error) => format!("Failed · {error}"),
        }
    }

    pub fn is_running(&self) -> bool {
        matches!(self, Self::Running { .. })
    }
}

#[derive(Default)]
pub struct Manager {
    processes: BTreeMap<String, TunnelProcess>,
    failures: BTreeMap<String, String>,
    /// Per-start acknowledgement for non-loopback binds, keyed by profile.
    /// UI state only; never persisted (docs/tunnels.md).
    acknowledged: BTreeMap<String, bool>,
}

impl Manager {
    pub fn status(&mut self, profile_key: &str) -> Status {
        if let Some(process) = self.processes.get_mut(profile_key) {
            match process.is_running() {
                Ok(true) => return Status::Running { pid: process.id() },
                Ok(false) => {
                    self.processes.remove(profile_key);
                    self.failures
                        .insert(profile_key.to_owned(), "ssh exited".to_owned());
                }
                Err(error) => {
                    self.processes.remove(profile_key);
                    self.failures
                        .insert(profile_key.to_owned(), format!("{error:#}"));
                }
            }
        }
        match self.failures.get(profile_key) {
            Some(error) => Status::Failed(error.clone()),
            None => Status::Stopped,
        }
    }

    pub fn running_count(&mut self) -> usize {
        let keys: Vec<String> = self.processes.keys().cloned().collect();
        keys.into_iter()
            .filter(|key| self.status(key).is_running())
            .count()
    }

    pub fn acknowledged(&self, profile_key: &str) -> bool {
        self.acknowledged.get(profile_key).copied().unwrap_or(false)
    }

    pub fn set_acknowledged(&mut self, profile_key: &str, value: bool) {
        self.acknowledged.insert(profile_key.to_owned(), value);
    }

    /// Start the forwarding process for one profile. Fails closed when the
    /// profile binds beyond loopback and the user has not acknowledged it.
    pub fn start(&mut self, session: &Session, config: Option<&Path>) -> Result<Status> {
        let key = session_profile_key(session);
        if self.status(&key).is_running() {
            return Ok(self.status(&key));
        }
        ensure!(
            !session_requires_forward_risk_ack(session) || self.acknowledged(&key),
            "non-loopback tunnel binds require explicit risk acknowledgement"
        );
        self.failures.remove(&key);
        match crate::terminal::start_tunnels(session, config) {
            Ok(process) => {
                let pid = process.id();
                self.processes.insert(key.clone(), process);
                // Acknowledgement is per start, as in the previous manager.
                self.acknowledged.remove(&key);
                Ok(Status::Running { pid })
            }
            Err(error) => {
                let message = format!("{error:#}");
                self.failures.insert(key, message.clone());
                Ok(Status::Failed(message))
            }
        }
    }

    pub fn stop(&mut self, profile_key: &str) -> Result<Status> {
        if let Some(mut process) = self.processes.remove(profile_key) {
            process.stop()?;
        }
        self.failures.remove(profile_key);
        Ok(Status::Stopped)
    }

    pub fn stop_all(&mut self) {
        let keys: Vec<String> = self.processes.keys().cloned().collect();
        for key in keys {
            let _ = self.stop(&key);
        }
    }
}

/// Forward specs separated by newlines or `;` (the connection dialog uses a
/// single-line input), blanks ignored.
pub fn parse_forward_lines(value: &str) -> Vec<String> {
    value
        .split(|c| c == '\n' || c == ';')
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn specs_split_into_listener_and_target() {
        assert_eq!(
            split_spec(ForwardKind::Local, "127.0.0.1:8080:internal.example:80"),
            ("127.0.0.1:8080".into(), "internal.example:80".into())
        );
        assert_eq!(
            split_spec(ForwardKind::Local, "8080:internal.example:80"),
            ("8080".into(), "internal.example:80".into())
        );
        assert_eq!(
            split_spec(ForwardKind::Remote, "[::1]:9000:[::1]:3000"),
            ("[::1]:9000".into(), "[::1]:3000".into())
        );
        assert_eq!(
            split_spec(ForwardKind::Dynamic, "127.0.0.1:1080"),
            ("127.0.0.1:1080".into(), String::new())
        );
    }

    #[test]
    fn rows_cover_every_profile_in_order() {
        let mut a = Session::default();
        a.name = "a".into();
        a.host = "a.example".into();
        a.ssh.local_forwards = vec!["127.0.0.1:8080:web:80".into()];
        a.ssh.dynamic_forwards = vec!["127.0.0.1:1080".into()];
        let mut b = Session::default();
        b.name = "b".into();
        b.host = "b.example".into();
        b.ssh.remote_forwards = vec!["127.0.0.1:9000:127.0.0.1:3000".into()];
        let rows = all_rows(&[a, b]);
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].kind, ForwardKind::Local);
        assert_eq!(rows[1].kind, ForwardKind::Dynamic);
        assert_eq!(rows[2].profile_name, "b");
        assert_eq!(rows[2].listen, "127.0.0.1:9000");
    }

    #[test]
    fn non_loopback_bind_fails_closed_without_acknowledgement() {
        let mut session = Session::default();
        session.name = "exposed".into();
        session.host = "h.example".into();
        session.ssh.local_forwards = vec!["0.0.0.0:8080:web:80".into()];
        let mut manager = Manager::default();
        let error = manager.start(&session, None).unwrap_err();
        assert!(error.to_string().contains("acknowledgement"));
        assert_eq!(manager.status("exposed"), Status::Stopped);
    }

    #[test]
    fn status_starts_stopped_and_stop_clears_failures() {
        let mut manager = Manager::default();
        assert_eq!(manager.status("none"), Status::Stopped);
        manager.failures.insert("x".into(), "boom".into());
        assert_eq!(manager.status("x"), Status::Failed("boom".into()));
        assert_eq!(manager.stop("x").unwrap(), Status::Stopped);
        assert_eq!(manager.status("x"), Status::Stopped);
        assert_eq!(manager.running_count(), 0);
    }

    #[test]
    fn forward_lines_trim_and_skip_blanks() {
        assert_eq!(
            parse_forward_lines(" a:1:b:2 \n\n c:3:d:4\n"),
            vec!["a:1:b:2".to_owned(), "c:3:d:4".to_owned()]
        );
        assert_eq!(
            parse_forward_lines("a:1:b:2 ; c:3:d:4;"),
            vec!["a:1:b:2".to_owned(), "c:3:d:4".to_owned()]
        );
    }
}
