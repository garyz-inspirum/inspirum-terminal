//! Centralised SSH tunnel manager model.
//!
//! Forwarding-only OpenSSH processes can own either a whole saved profile or
//! one saved forward. The manager tracks their lifetimes independently.
//! It deliberately contains no Iced types so it can be unit tested.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Result, ensure};

use crate::terminal::TunnelProcess;
use crate::{
    Session, TunnelKind, TunnelLabel, session_profile_key, session_requires_forward_risk_ack,
};

/// Which OpenSSH forwarding flag a row came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ForwardKind {
    Local,
    Remote,
    Dynamic,
}

impl From<ForwardKind> for TunnelKind {
    fn from(value: ForwardKind) -> Self {
        match value {
            ForwardKind::Local => TunnelKind::Local,
            ForwardKind::Remote => TunnelKind::Remote,
            ForwardKind::Dynamic => TunnelKind::Dynamic,
        }
    }
}

impl From<TunnelKind> for ForwardKind {
    fn from(value: TunnelKind) -> Self {
        match value {
            TunnelKind::Local => ForwardKind::Local,
            TunnelKind::Remote => ForwardKind::Remote,
            TunnelKind::Dynamic => ForwardKind::Dynamic,
        }
    }
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
    pub label: String,
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
                label: label_for(session, kind, spec),
            });
        }
    }
    rows
}

pub fn label_for(session: &Session, kind: ForwardKind, spec: &str) -> String {
    session
        .ssh
        .tunnel_labels
        .iter()
        .find(|meta| meta.kind == TunnelKind::from(kind) && meta.spec == spec)
        .map(|meta| meta.label.trim().to_owned())
        .filter(|label| !label.is_empty())
        .unwrap_or_else(|| default_label(kind, spec))
}

fn default_label(kind: ForwardKind, spec: &str) -> String {
    let (listen, target) = split_spec(kind, spec);
    match kind {
        ForwardKind::Dynamic => format!("SOCKS {listen}"),
        ForwardKind::Local => format!("{listen} -> {target}"),
        ForwardKind::Remote => format!("{listen} <- {target}"),
    }
}

pub fn set_label(session: &mut Session, kind: ForwardKind, spec: &str, label: &str) {
    session
        .ssh
        .tunnel_labels
        .retain(|meta| !(meta.kind == TunnelKind::from(kind) && meta.spec == spec));
    let label = label.trim();
    if !label.is_empty() {
        session.ssh.tunnel_labels.push(TunnelLabel {
            kind: kind.into(),
            spec: spec.to_owned(),
            label: label.to_owned(),
        });
    }
}

fn specs_mut(session: &mut Session, kind: ForwardKind) -> &mut Vec<String> {
    match kind {
        ForwardKind::Local => &mut session.ssh.local_forwards,
        ForwardKind::Remote => &mut session.ssh.remote_forwards,
        ForwardKind::Dynamic => &mut session.ssh.dynamic_forwards,
    }
}

pub fn add_forward(
    session: &mut Session,
    kind: ForwardKind,
    spec: String,
    label: &str,
) -> Result<()> {
    ensure!(
        !spec.trim().is_empty(),
        "tunnel specification cannot be empty"
    );
    let mut candidate = session.clone();
    specs_mut(&mut candidate, kind).push(spec.clone());
    set_label(&mut candidate, kind, &spec, label);
    candidate.ssh_args()?;
    *session = candidate;
    Ok(())
}

pub fn replace_forward(
    session: &mut Session,
    old_kind: ForwardKind,
    old_spec: &str,
    kind: ForwardKind,
    spec: String,
    label: &str,
) -> Result<()> {
    let mut candidate = session.clone();
    let old_specs = specs_mut(&mut candidate, old_kind);
    let index = old_specs
        .iter()
        .position(|value| value == old_spec)
        .ok_or_else(|| anyhow::anyhow!("tunnel entry no longer exists"))?;
    old_specs.remove(index);
    candidate
        .ssh
        .tunnel_labels
        .retain(|meta| !(meta.kind == TunnelKind::from(old_kind) && meta.spec == old_spec));
    specs_mut(&mut candidate, kind).push(spec.clone());
    set_label(&mut candidate, kind, &spec, label);
    candidate.ssh_args()?;
    *session = candidate;
    Ok(())
}

pub fn remove_forward(session: &mut Session, kind: ForwardKind, spec: &str) -> Result<()> {
    let mut candidate = session.clone();
    let specs = specs_mut(&mut candidate, kind);
    let index = specs
        .iter()
        .position(|value| value == spec)
        .ok_or_else(|| anyhow::anyhow!("tunnel entry no longer exists"))?;
    specs.remove(index);
    candidate
        .ssh
        .tunnel_labels
        .retain(|meta| !(meta.kind == TunnelKind::from(kind) && meta.spec == spec));
    candidate.ssh_args()?;
    *session = candidate;
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ForwardDraft {
    pub kind: ForwardKind,
    pub label: String,
    pub bind_address: String,
    pub listen_port: String,
    pub destination_host: String,
    pub destination_port: String,
}

impl Default for ForwardDraft {
    fn default() -> Self {
        Self {
            kind: ForwardKind::Local,
            label: String::new(),
            bind_address: "127.0.0.1".into(),
            listen_port: String::new(),
            destination_host: String::new(),
            destination_port: String::new(),
        }
    }
}

impl ForwardDraft {
    pub fn from_row(row: &ForwardRow) -> Self {
        let (bind_address, listen_port) = split_listener(&row.listen);
        let (destination_host, destination_port) = split_target(&row.target);
        Self {
            kind: row.kind,
            label: row.label.clone(),
            bind_address,
            listen_port,
            destination_host,
            destination_port,
        }
    }

    pub fn spec(&self) -> Result<String> {
        let port = parse_port("listen port", &self.listen_port)?;
        let bind = self.bind_address.trim();
        ensure!(
            bind.len() <= 253 && !bind.chars().any(char::is_control),
            "bind address is invalid"
        );
        let listen = if bind.is_empty() {
            port.to_string()
        } else {
            format!("{bind}:{port}")
        };
        if self.kind == ForwardKind::Dynamic {
            return Ok(listen);
        }
        let host = self.destination_host.trim();
        ensure!(!host.is_empty(), "destination host is required");
        ensure!(
            host.len() <= 253
                && !host.chars().any(char::is_control)
                && !host.chars().any(char::is_whitespace),
            "destination host is invalid"
        );
        let destination_port = parse_port("destination port", &self.destination_port)?;
        Ok(format!("{listen}:{host}:{destination_port}"))
    }
}

fn parse_port(name: &str, value: &str) -> Result<u16> {
    let port: u16 = value
        .trim()
        .parse()
        .map_err(|_| anyhow::anyhow!("{name} must be a number from 1 to 65535"))?;
    ensure!(port > 0, "{name} must be from 1 to 65535");
    Ok(port)
}

fn split_listener(value: &str) -> (String, String) {
    if let Some((bind, port)) = value.rsplit_once(':') {
        (bind.to_owned(), port.to_owned())
    } else {
        (String::new(), value.to_owned())
    }
}

fn split_target(value: &str) -> (String, String) {
    if value.is_empty() {
        return (String::new(), String::new());
    }
    if let Some((host, port)) = value.rsplit_once(':') {
        (host.to_owned(), port.to_owned())
    } else {
        (value.to_owned(), String::new())
    }
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

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum ProcessKey {
    Profile(String),
    Forward {
        profile: String,
        kind: ForwardKind,
        spec: String,
    },
}

impl ProcessKey {
    fn forward(profile: &str, kind: ForwardKind, spec: &str) -> Self {
        Self::Forward {
            profile: profile.to_owned(),
            kind,
            spec: spec.to_owned(),
        }
    }

    fn belongs_to(&self, profile: &str) -> bool {
        match self {
            Self::Profile(key) => key == profile,
            Self::Forward { profile: key, .. } => key == profile,
        }
    }
}

#[derive(Default)]
pub struct Manager {
    processes: BTreeMap<ProcessKey, TunnelProcess>,
    failures: BTreeMap<ProcessKey, String>,
    /// Per-start acknowledgement for non-loopback binds, keyed by profile.
    /// UI state only; never persisted (docs/tunnels.md).
    acknowledged: BTreeMap<String, bool>,
}

impl Manager {
    fn status_for(&mut self, key: &ProcessKey) -> Status {
        if let Some(process) = self.processes.get_mut(key) {
            match process.is_running() {
                Ok(true) => return Status::Running { pid: process.id() },
                Ok(false) => {
                    self.processes.remove(key);
                    self.failures.insert(key.clone(), "ssh exited".into());
                }
                Err(error) => {
                    self.processes.remove(key);
                    self.failures.insert(key.clone(), format!("{error:#}"));
                }
            }
        }
        self.failures
            .get(key)
            .map_or(Status::Stopped, |error| Status::Failed(error.clone()))
    }

    /// Status of the legacy aggregate profile process only. Individual
    /// forwards must not trigger UI edit paths that stop the entire profile.
    pub fn status(&mut self, profile_key: &str) -> Status {
        self.status_for(&ProcessKey::Profile(profile_key.to_owned()))
    }

    pub fn forward_status(&mut self, profile_key: &str, kind: ForwardKind, spec: &str) -> Status {
        let key = ProcessKey::forward(profile_key, kind, spec);
        let own = self.status_for(&key);
        if own.is_running() {
            return own;
        }
        let aggregate = self.status_for(&ProcessKey::Profile(profile_key.to_owned()));
        if aggregate.is_running() {
            aggregate
        } else {
            own
        }
    }

    pub fn running_count(&mut self) -> usize {
        let keys: Vec<_> = self.processes.keys().cloned().collect();
        keys.into_iter()
            .filter(|key| self.status_for(key).is_running())
            .count()
    }

    pub fn acknowledged(&self, profile_key: &str) -> bool {
        self.acknowledged.get(profile_key).copied().unwrap_or(false)
    }

    pub fn set_acknowledged(&mut self, profile_key: &str, value: bool) {
        self.acknowledged.insert(profile_key.to_owned(), value);
    }

    /// The aggregate process cannot coexist with independently managed rows:
    /// stopping one row must never kill an unrelated listener.
    pub fn start(&mut self, session: &Session, config: Option<&Path>) -> Result<Status> {
        let profile = session_profile_key(session);
        let key = ProcessKey::Profile(profile.clone());
        if self.status_for(&key).is_running() {
            return Ok(self.status_for(&key));
        }
        let active: Vec<_> = self
            .processes
            .keys()
            .filter(|key| key.belongs_to(&profile))
            .cloned()
            .collect();
        for other in active {
            ensure!(
                !self.status_for(&other).is_running(),
                "stop individual forwards before starting the whole profile"
            );
        }
        ensure!(
            !session_requires_forward_risk_ack(session) || self.acknowledged(&profile),
            "non-loopback tunnel binds require explicit risk acknowledgement"
        );
        self.failures.remove(&key);
        match crate::terminal::start_tunnels(session, config) {
            Ok(process) => {
                let pid = process.id();
                self.processes.insert(key, process);
                self.acknowledged.remove(&profile);
                Ok(Status::Running { pid })
            }
            Err(error) => {
                let message = format!("{error:#}");
                self.failures.insert(key, message.clone());
                Ok(Status::Failed(message))
            }
        }
    }

    /// Start exactly one saved forward, leaving every other process untouched.
    pub fn start_forward(
        &mut self,
        session: &Session,
        kind: ForwardKind,
        spec: &str,
        config: Option<&Path>,
    ) -> Result<Status> {
        let profile = session_profile_key(session);
        let key = ProcessKey::forward(&profile, kind, spec);
        session.ssh_args()?;
        ensure!(
            rows_for(session)
                .iter()
                .any(|row| row.kind == kind && row.spec == spec),
            "tunnel entry no longer exists"
        );
        if self.status_for(&key).is_running() {
            return Ok(self.status_for(&key));
        }
        ensure!(
            !self
                .status_for(&ProcessKey::Profile(profile.clone()))
                .is_running(),
            "stop the whole profile before starting an individual forward"
        );
        // Reject conflicting listeners before spawning; OpenSSH also enforces
        // ExitOnForwardFailure for conflicts with external processes.
        let listener = split_spec(kind, spec).0;
        let active: Vec<_> = self.processes.keys().cloned().collect();
        for other in active {
            if !self.status_for(&other).is_running() {
                continue;
            }
            if let ProcessKey::Forward {
                kind: other_kind,
                spec: other_spec,
                ..
            } = &other
            {
                let same_side =
                    (kind == ForwardKind::Remote) == (*other_kind == ForwardKind::Remote);
                ensure!(
                    !same_side || split_spec(*other_kind, other_spec).0 != listener,
                    "tunnel listener is already managed by another forward"
                );
            }
        }
        let selected = isolated_forward(session, kind, spec);
        // Assess only the selected bind; unrelated exposed forwards do not
        // require acknowledgement for this launch.
        ensure!(
            !session_requires_forward_risk_ack(&selected) || self.acknowledged(&profile),
            "non-loopback tunnel binds require explicit risk acknowledgement"
        );
        selected.ssh_args()?;
        self.failures.remove(&key);
        match crate::terminal::start_tunnels(&selected, config) {
            Ok(process) => {
                let pid = process.id();
                self.processes.insert(key, process);
                self.acknowledged.remove(&profile);
                Ok(Status::Running { pid })
            }
            Err(error) => {
                let message = format!("{error:#}");
                self.failures.insert(key, message.clone());
                Ok(Status::Failed(message))
            }
        }
    }

    pub fn stop_forward(
        &mut self,
        profile_key: &str,
        kind: ForwardKind,
        spec: &str,
    ) -> Result<Status> {
        let key = ProcessKey::forward(profile_key, kind, spec);
        ensure!(
            !self
                .status_for(&ProcessKey::Profile(profile_key.to_owned()))
                .is_running(),
            "stop the whole profile before stopping an individual forward"
        );
        self.stop_key(&key)?;
        Ok(Status::Stopped)
    }

    fn stop_key(&mut self, key: &ProcessKey) -> Result<()> {
        if let Some(mut process) = self.processes.remove(key) {
            process.stop()?;
        }
        self.failures.remove(key);
        Ok(())
    }

    pub fn stop(&mut self, profile_key: &str) -> Result<Status> {
        let keys: Vec<_> = self
            .processes
            .keys()
            .chain(self.failures.keys())
            .filter(|key| key.belongs_to(profile_key))
            .cloned()
            .collect();
        for key in keys {
            self.stop_key(&key)?;
        }
        Ok(Status::Stopped)
    }

    pub fn stop_all(&mut self) {
        let keys: Vec<_> = self.processes.keys().cloned().collect();
        for key in keys {
            let _ = self.stop_key(&key);
        }
        self.failures.clear();
    }
}

fn isolated_forward(session: &Session, kind: ForwardKind, spec: &str) -> Session {
    let mut selected = session.clone();
    selected.ssh.local_forwards.clear();
    selected.ssh.remote_forwards.clear();
    selected.ssh.dynamic_forwards.clear();
    specs_mut(&mut selected, kind).push(spec.to_owned());
    selected
}

/// Forward specs separated by newlines or `;` (the connection dialog uses a
/// single-line input), blanks ignored.
pub fn parse_forward_lines(value: &str) -> Vec<String> {
    value
        .split(['\n', ';'])
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
        let a = Session {
            name: "a".into(),
            host: "a.example".into(),
            ssh: crate::SshOptions {
                local_forwards: vec!["127.0.0.1:8080:web:80".into()],
                dynamic_forwards: vec!["127.0.0.1:1080".into()],
                ..Default::default()
            },
            ..Default::default()
        };
        let b = Session {
            name: "b".into(),
            host: "b.example".into(),
            ssh: crate::SshOptions {
                remote_forwards: vec!["127.0.0.1:9000:127.0.0.1:3000".into()],
                ..Default::default()
            },
            ..Default::default()
        };
        let rows = all_rows(&[a, b]);
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].kind, ForwardKind::Local);
        assert_eq!(rows[1].kind, ForwardKind::Dynamic);
        assert_eq!(rows[2].profile_name, "b");
        assert_eq!(rows[2].listen, "127.0.0.1:9000");
    }

    #[test]
    fn non_loopback_bind_fails_closed_without_acknowledgement() {
        let session = Session {
            name: "exposed".into(),
            host: "h.example".into(),
            ssh: crate::SshOptions {
                local_forwards: vec!["0.0.0.0:8080:web:80".into()],
                ..Default::default()
            },
            ..Default::default()
        };
        let mut manager = Manager::default();
        let error = manager.start(&session, None).unwrap_err();
        assert!(error.to_string().contains("acknowledgement"));
        assert_eq!(manager.status("exposed"), Status::Stopped);
    }

    #[test]
    fn status_starts_stopped_and_stop_clears_failures() {
        let mut manager = Manager::default();
        assert_eq!(manager.status("none"), Status::Stopped);
        manager
            .failures
            .insert(ProcessKey::Profile("x".into()), "boom".into());
        assert_eq!(manager.status("x"), Status::Failed("boom".into()));
        assert_eq!(manager.stop("x").unwrap(), Status::Stopped);
        assert_eq!(manager.status("x"), Status::Stopped);
        assert_eq!(manager.running_count(), 0);
    }

    #[test]
    fn individual_state_is_isolated_by_profile_kind_and_spec() {
        let mut manager = Manager::default();
        let a = ForwardKind::Local;
        let b = ForwardKind::Dynamic;
        manager.failures.insert(
            ProcessKey::forward("one", a, "127.0.0.1:8000:web:80"),
            "failed".into(),
        );
        assert_eq!(
            manager.forward_status("one", a, "127.0.0.1:8000:web:80"),
            Status::Failed("failed".into())
        );
        assert_eq!(
            manager.forward_status("two", a, "127.0.0.1:8000:web:80"),
            Status::Stopped
        );
        assert_eq!(
            manager.forward_status("one", b, "127.0.0.1:8000:web:80"),
            Status::Stopped
        );
        manager
            .stop_forward("one", b, "127.0.0.1:8000:web:80")
            .unwrap();
        assert_eq!(
            manager.forward_status("one", a, "127.0.0.1:8000:web:80"),
            Status::Failed("failed".into())
        );
        manager.stop("one").unwrap();
        assert_eq!(
            manager.forward_status("one", a, "127.0.0.1:8000:web:80"),
            Status::Stopped
        );
    }

    #[test]
    fn isolated_forward_rejects_absent_spec_without_launching() {
        let session = Session {
            name: "srv".into(),
            host: "srv.example".into(),
            ..Default::default()
        };
        let mut manager = Manager::default();
        assert!(
            manager
                .start_forward(&session, ForwardKind::Local, "127.0.0.1:8080:web:80", None)
                .is_err()
        );
        assert_eq!(manager.running_count(), 0);
    }

    #[test]
    fn isolated_launch_preserves_ssh_options_and_only_selected_listener() {
        let session = Session {
            name: "srv".into(),
            host: "srv.example".into(),
            ssh: crate::SshOptions {
                local_forwards: vec![
                    "127.0.0.1:8080:web:80".into(),
                    "127.0.0.1:8081:db:5432".into(),
                ],
                dynamic_forwards: vec!["127.0.0.1:1080".into()],
                ..Default::default()
            },
            ..Default::default()
        };
        let selected = isolated_forward(&session, ForwardKind::Local, "127.0.0.1:8081:db:5432");
        let argv = crate::terminal::tunnel_launch_args(&selected, None).unwrap();
        assert!(
            argv.windows(2)
                .any(|pair| pair[0] == "-L" && pair[1] == "127.0.0.1:8081:db:5432")
        );
        assert!(
            !argv
                .iter()
                .any(|arg| arg == "127.0.0.1:8080:web:80" || arg == "127.0.0.1:1080")
        );
        assert!(argv.iter().any(|arg| arg == "ExitOnForwardFailure=yes"));
        assert!(argv.iter().any(|arg| arg == "-N"));
        assert_eq!(selected.host, session.host);
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

    #[test]
    fn structured_draft_round_trips_local_forward() {
        let row = ForwardRow {
            profile_key: "p".into(),
            profile_name: "p".into(),
            kind: ForwardKind::Local,
            listen: "127.0.0.1:8080".into(),
            target: "db.internal:5432".into(),
            spec: "127.0.0.1:8080:db.internal:5432".into(),
            label: "Database".into(),
        };
        let draft = ForwardDraft::from_row(&row);
        assert_eq!(draft.label, "Database");
        assert_eq!(draft.spec().unwrap(), row.spec);
    }

    #[test]
    fn labels_follow_add_replace_remove() {
        let mut session = Session {
            name: "srv".into(),
            host: "srv.example".into(),
            ..Default::default()
        };
        add_forward(
            &mut session,
            ForwardKind::Local,
            "127.0.0.1:8080:web:80".into(),
            "Web",
        )
        .unwrap();
        assert_eq!(rows_for(&session)[0].label, "Web");
        replace_forward(
            &mut session,
            ForwardKind::Local,
            "127.0.0.1:8080:web:80",
            ForwardKind::Remote,
            "127.0.0.1:9000:127.0.0.1:3000".into(),
            "Remote API",
        )
        .unwrap();
        let rows = rows_for(&session);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].kind, ForwardKind::Remote);
        assert_eq!(rows[0].label, "Remote API");
        remove_forward(
            &mut session,
            ForwardKind::Remote,
            "127.0.0.1:9000:127.0.0.1:3000",
        )
        .unwrap();
        assert!(rows_for(&session).is_empty());
        assert!(session.ssh.tunnel_labels.is_empty());
    }
}
