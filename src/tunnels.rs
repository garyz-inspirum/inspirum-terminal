//! Centralised SSH tunnel manager model.
//!
//! Forwarding-only OpenSSH processes can own either a whole saved profile or
//! one saved forward. The manager tracks their lifetimes independently.
//! It deliberately contains no Iced types so it can be unit tested.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Result, ensure};

use crate::terminal::{EffectiveForwards, TunnelProcess};
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
        .find(|meta| matches_meta(meta, kind, spec))
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

fn matches_meta(meta: &TunnelLabel, kind: ForwardKind, spec: &str) -> bool {
    meta.kind == TunnelKind::from(kind) && meta.spec == spec
}

/// First metadata row for a forward, mirroring `label_for`'s first-match rule.
fn meta_for<'a>(session: &'a Session, kind: ForwardKind, spec: &str) -> Option<&'a TunnelLabel> {
    session
        .ssh
        .tunnel_labels
        .iter()
        .find(|meta| matches_meta(meta, kind, spec))
}

fn forward_exists(session: &Session, kind: ForwardKind, spec: &str) -> bool {
    let specs = match kind {
        ForwardKind::Local => &session.ssh.local_forwards,
        ForwardKind::Remote => &session.ssh.remote_forwards,
        ForwardKind::Dynamic => &session.ssh.dynamic_forwards,
    };
    specs.iter().any(|value| value == spec)
}

/// Replace every metadata row for `kind`/`spec` with one row, or with none
/// when it would carry nothing (empty label and auto-start off).
fn write_meta(session: &mut Session, kind: ForwardKind, spec: &str, label: &str, auto_start: bool) {
    session
        .ssh
        .tunnel_labels
        .retain(|meta| !matches_meta(meta, kind, spec));
    let label = label.trim();
    if !label.is_empty() || auto_start {
        session.ssh.tunnel_labels.push(TunnelLabel {
            kind: kind.into(),
            spec: spec.to_owned(),
            label: label.to_owned(),
            auto_start,
        });
    }
}

/// Set a forward's label, keeping its auto-start opt-in unchanged.
pub fn set_label(session: &mut Session, kind: ForwardKind, spec: &str, label: &str) {
    let auto_start = meta_for(session, kind, spec).is_some_and(|meta| meta.auto_start);
    write_meta(session, kind, spec, label, auto_start);
}

/// Whether a saved forward is opted in to start when the app opens.
/// Metadata whose forward no longer exists is ignored.
pub fn auto_start_for(session: &Session, kind: ForwardKind, spec: &str) -> bool {
    forward_exists(session, kind, spec)
        && meta_for(session, kind, spec).is_some_and(|meta| meta.auto_start)
}

/// Saved forwards opted in to auto-start, in `rows_for` order.
pub fn auto_start_forwards(session: &Session) -> Vec<(ForwardKind, String)> {
    rows_for(session)
        .into_iter()
        .filter(|row| auto_start_for(session, row.kind, &row.spec))
        .map(|row| (row.kind, row.spec))
        .collect()
}

/// Whether a forward may be launched unattended at app start.
///
/// Stricter than [`session_requires_forward_risk_ack`] on purpose: auto-start
/// never shows the risk prompt, so the listener must be loopback whatever the
/// SSH configuration says.
/// - Only Local and Dynamic qualify. A Remote listener is bound by the SSH
///   server, whose `GatewayPorts yes` overrides the requested bind address,
///   so the client cannot guarantee loopback; remote forwards start manually.
/// - The bind must be an explicit numeric loopback (`127.0.0.1:` or
///   `[::1]:`). An omitted bind follows client `GatewayPorts` (ssh_config can
///   widen it to the wildcard address) and `localhost` follows name
///   resolution (`/etc/hosts`), so neither counts as loopback-only here.
/// - The listen port is a decimal 1-65535, so the listener is TCP on that
///   address (not a socket path or another bind smuggled after the prefix).
pub fn auto_start_eligible(kind: ForwardKind, spec: &str) -> bool {
    if kind == ForwardKind::Remote {
        return false;
    }
    let Some(rest) = spec
        .strip_prefix("127.0.0.1:")
        .or_else(|| spec.strip_prefix("[::1]:"))
    else {
        return false;
    };
    let (port, target) = match kind {
        ForwardKind::Dynamic => (rest, None),
        _ => match rest.split_once(':') {
            Some((port, target)) => (port, Some(target)),
            None => return false,
        },
    };
    let port_ok = !port.is_empty()
        && port.len() <= 5
        && port.bytes().all(|byte| byte.is_ascii_digit())
        && port.parse::<u16>().is_ok_and(|port| port != 0);
    port_ok && target.is_none_or(|target| !target.is_empty())
}

/// Opt a saved forward in or out of auto-start, keeping its label.
/// Rejects a forward that is not saved so no dangling metadata is created,
/// and refuses to opt in a forward that is not [`auto_start_eligible`].
pub fn set_auto_start(
    session: &mut Session,
    kind: ForwardKind,
    spec: &str,
    enabled: bool,
) -> Result<()> {
    ensure!(
        forward_exists(session, kind, spec),
        "tunnel entry no longer exists"
    );
    ensure!(
        !enabled || auto_start_eligible(kind, spec),
        "only local or dynamic tunnels bound to 127.0.0.1 or [::1] can auto-start"
    );
    let mut candidate = session.clone();
    let label = meta_for(&candidate, kind, spec)
        .map(|meta| meta.label.clone())
        .unwrap_or_default();
    write_meta(&mut candidate, kind, spec, &label, enabled);
    candidate.ssh_args()?;
    *session = candidate;
    Ok(())
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
    // Editing a row (label, kind or spec) keeps that row's auto-start opt-in.
    let auto_start = meta_for(&candidate, old_kind, old_spec).is_some_and(|meta| meta.auto_start);
    candidate
        .ssh
        .tunnel_labels
        .retain(|meta| !matches_meta(meta, old_kind, old_spec));
    specs_mut(&mut candidate, kind).push(spec.clone());
    write_meta(&mut candidate, kind, &spec, label, auto_start);
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
        .retain(|meta| !matches_meta(meta, kind, spec));
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

/// Remote (`-R`) listeners bind on the SSH server, so their port namespace
/// is per server; local and dynamic listeners all share this machine.
#[derive(Clone, Debug, PartialEq, Eq)]
struct ServerIdentity {
    host: String,
    port: Option<u16>,
}

impl ServerIdentity {
    fn of(session: &Session) -> Self {
        Self {
            host: session.host.trim().to_ascii_lowercase(),
            port: session.port,
        }
    }
}

#[derive(Default)]
pub struct Manager {
    processes: BTreeMap<ProcessKey, TunnelProcess>,
    /// Server each managed process connected to, for remote conflicts.
    servers: BTreeMap<ProcessKey, ServerIdentity>,
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
                    self.servers.remove(key);
                    self.failures.insert(key.clone(), "ssh exited".into());
                }
                Err(error) => {
                    self.processes.remove(key);
                    self.servers.remove(key);
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
        self.launch_forward(session, kind, spec, config, None)
    }

    /// [`Self::start_forward`] for unattended launches: before spawning, ask
    /// OpenSSH (`ssh -G`) which forwards the launch would really set up and
    /// refuse unless that is exactly the selected one.
    pub fn start_forward_exact(
        &mut self,
        session: &Session,
        kind: ForwardKind,
        spec: &str,
        config: Option<&Path>,
    ) -> Result<Status> {
        let preflight: Preflight<'_> = &crate::terminal::effective_forwards;
        self.launch_forward(session, kind, spec, config, Some(preflight))
    }

    /// Shared body of the start variants. `preflight` runs strictly before
    /// `start_tunnels`; its failure returns `Err` with nothing spawned or
    /// recorded.
    fn launch_forward(
        &mut self,
        session: &Session,
        kind: ForwardKind,
        spec: &str,
        config: Option<&Path>,
        preflight: Option<Preflight<'_>>,
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
        // Remote listeners only conflict on the same SSH server.
        let listener = split_spec(kind, spec).0;
        let server = ServerIdentity::of(session);
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
                let same_namespace = match (
                    kind == ForwardKind::Remote,
                    *other_kind == ForwardKind::Remote,
                ) {
                    (false, false) => true,
                    // Unknown server: stay conservative and treat as shared.
                    (true, true) => self
                        .servers
                        .get(&other)
                        .is_none_or(|other| *other == server),
                    _ => false,
                };
                ensure!(
                    !same_namespace || split_spec(*other_kind, other_spec).0 != listener,
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
        if let Some(preflight) = preflight {
            let effective = preflight(&selected, config)?;
            ensure_exact_forward(&effective, kind, spec)?;
        }
        self.failures.remove(&key);
        match crate::terminal::start_tunnels(&selected, config) {
            Ok(process) => {
                let pid = process.id();
                self.servers.insert(key.clone(), server);
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
        self.servers.remove(key);
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

/// Effective-forward query run before an unattended spawn.
type Preflight<'a> = &'a dyn Fn(&Session, Option<&Path>) -> Result<EffectiveForwards>;

/// The OpenSSH configuration would make an unattended launch set up more,
/// fewer or different forwards than the selected one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InheritedForwards;

impl std::fmt::Display for InheritedForwards {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(
            "OpenSSH configuration for this host adds or changes port forwards; \
start this tunnel manually",
        )
    }
}

impl std::error::Error for InheritedForwards {}

/// Ok only when `effective` (from `ssh -G`) is exactly the selected forward.
pub fn ensure_exact_forward(
    effective: &EffectiveForwards,
    kind: ForwardKind,
    spec: &str,
) -> Result<()> {
    // Seam only: enforcement lands with the fix.
    let _ = (effective, kind, spec);
    Ok(())
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

    fn web_session() -> Session {
        let mut session = Session {
            name: "srv".into(),
            host: "srv.example".into(),
            ..Default::default()
        };
        add_forward(
            &mut session,
            ForwardKind::Local,
            "127.0.0.1:8080:web:80".into(),
            "",
        )
        .unwrap();
        session
    }

    const WEB: &str = "127.0.0.1:8080:web:80";

    #[test]
    fn old_profile_metadata_defaults_auto_start_off_and_off_is_omitted() {
        let meta: TunnelLabel = serde_json::from_str(
            r#"{"kind":"local","spec":"127.0.0.1:8080:web:80","label":"Web"}"#,
        )
        .unwrap();
        assert!(!meta.auto_start);
        assert_eq!(meta.label, "Web");
        let json = serde_json::to_string(&meta).unwrap();
        assert!(!json.contains("auto_start"), "{json}");
        let mut session = web_session();
        session.ssh.tunnel_labels.push(meta);
        assert!(!auto_start_for(&session, ForwardKind::Local, WEB));
        assert!(auto_start_forwards(&session).is_empty());
    }

    #[test]
    fn unknown_or_malformed_metadata_fields_are_rejected() {
        let unknown = r#"{"kind":"local","spec":"127.0.0.1:8080:web:80","label":"","auto_start":true,"run_command":"rm -rf /"}"#;
        assert!(serde_json::from_str::<TunnelLabel>(unknown).is_err());
        let wrong_type =
            r#"{"kind":"local","spec":"127.0.0.1:8080:web:80","label":"","auto_start":"yes"}"#;
        assert!(serde_json::from_str::<TunnelLabel>(wrong_type).is_err());
        let ok = r#"{"kind":"local","spec":"127.0.0.1:8080:web:80","label":"","auto_start":true}"#;
        assert!(serde_json::from_str::<TunnelLabel>(ok).unwrap().auto_start);
    }

    #[test]
    fn unlabeled_auto_start_survives_save_and_reload() {
        let mut session = web_session();
        assert!(session.ssh.tunnel_labels.is_empty());
        set_auto_start(&mut session, ForwardKind::Local, WEB, true).unwrap();
        assert_eq!(session.ssh.tunnel_labels.len(), 1);
        assert!(session.ssh.tunnel_labels[0].label.is_empty());
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sessions.json");
        crate::save_sessions(&path, std::slice::from_ref(&session)).unwrap();
        let loaded = crate::load_sessions(&path).unwrap();
        assert_eq!(loaded, vec![session]);
        assert!(auto_start_for(&loaded[0], ForwardKind::Local, WEB));
        assert_eq!(
            auto_start_forwards(&loaded[0]),
            vec![(ForwardKind::Local, WEB.to_owned())]
        );
        assert_eq!(rows_for(&loaded[0])[0].label, "127.0.0.1:8080 -> web:80");
    }

    #[test]
    fn label_edits_preserve_auto_start_and_disabling_keeps_label() {
        let mut session = web_session();
        set_auto_start(&mut session, ForwardKind::Local, WEB, true).unwrap();
        set_label(&mut session, ForwardKind::Local, WEB, "Web");
        assert!(auto_start_for(&session, ForwardKind::Local, WEB));
        assert_eq!(label_for(&session, ForwardKind::Local, WEB), "Web");
        replace_forward(
            &mut session,
            ForwardKind::Local,
            WEB,
            ForwardKind::Local,
            WEB.into(),
            "Web app",
        )
        .unwrap();
        assert!(auto_start_for(&session, ForwardKind::Local, WEB));
        assert_eq!(label_for(&session, ForwardKind::Local, WEB), "Web app");
        // Clearing the label keeps the row because it still carries auto-start.
        set_label(&mut session, ForwardKind::Local, WEB, "  ");
        assert_eq!(session.ssh.tunnel_labels.len(), 1);
        assert!(auto_start_for(&session, ForwardKind::Local, WEB));
        set_label(&mut session, ForwardKind::Local, WEB, "Web");
        set_auto_start(&mut session, ForwardKind::Local, WEB, false).unwrap();
        assert!(!auto_start_for(&session, ForwardKind::Local, WEB));
        assert_eq!(label_for(&session, ForwardKind::Local, WEB), "Web");
        set_label(&mut session, ForwardKind::Local, WEB, "");
        assert!(session.ssh.tunnel_labels.is_empty());
    }

    #[test]
    fn spec_change_carries_auto_start_to_the_edited_row_only() {
        let mut session = web_session();
        add_forward(
            &mut session,
            ForwardKind::Dynamic,
            "127.0.0.1:1080".into(),
            "Socks",
        )
        .unwrap();
        set_auto_start(&mut session, ForwardKind::Local, WEB, true).unwrap();
        let new_spec = "127.0.0.1:8081:web:80";
        replace_forward(
            &mut session,
            ForwardKind::Local,
            WEB,
            ForwardKind::Local,
            new_spec.into(),
            "",
        )
        .unwrap();
        assert!(auto_start_for(&session, ForwardKind::Local, new_spec));
        assert!(!auto_start_for(&session, ForwardKind::Local, WEB));
        assert!(!auto_start_for(
            &session,
            ForwardKind::Dynamic,
            "127.0.0.1:1080"
        ));
        assert!(
            session
                .ssh
                .tunnel_labels
                .iter()
                .all(|meta| meta.spec != WEB),
            "old spec metadata must not dangle"
        );
        assert_eq!(
            auto_start_forwards(&session),
            vec![(ForwardKind::Local, new_spec.to_owned())]
        );
    }

    #[test]
    fn removing_a_forward_drops_its_auto_start() {
        let mut session = web_session();
        set_auto_start(&mut session, ForwardKind::Local, WEB, true).unwrap();
        remove_forward(&mut session, ForwardKind::Local, WEB).unwrap();
        assert!(session.ssh.tunnel_labels.is_empty());
        assert!(!auto_start_for(&session, ForwardKind::Local, WEB));
        // Re-adding the same spec starts opted out.
        add_forward(&mut session, ForwardKind::Local, WEB.into(), "").unwrap();
        assert!(!auto_start_for(&session, ForwardKind::Local, WEB));
    }

    #[test]
    fn auto_start_rejects_unsaved_forwards_without_mutation() {
        let mut session = web_session();
        let before = session.clone();
        assert!(set_auto_start(&mut session, ForwardKind::Local, "127.0.0.1:9:x:9", true).is_err());
        // Same spec under another kind is a different forward.
        assert!(set_auto_start(&mut session, ForwardKind::Remote, WEB, true).is_err());
        assert_eq!(session, before);
        // Dangling metadata (forward edited out of the vector) is ignored.
        session.ssh.tunnel_labels.push(TunnelLabel {
            kind: TunnelKind::Local,
            spec: "127.0.0.1:9:x:9".into(),
            label: String::new(),
            auto_start: true,
        });
        assert!(!auto_start_for(
            &session,
            ForwardKind::Local,
            "127.0.0.1:9:x:9"
        ));
        assert!(auto_start_forwards(&session).is_empty());
    }

    #[test]
    fn auto_start_eligibility_requires_explicit_numeric_loopback_local_or_dynamic() {
        use ForwardKind::{Dynamic, Local, Remote};
        for (kind, spec) in [
            (Local, "127.0.0.1:8080:web:80"),
            (Local, "[::1]:8080:web:80"),
            (Local, "127.0.0.1:8080:/run/app.sock"),
            (Dynamic, "127.0.0.1:1080"),
            (Dynamic, "[::1]:1080"),
        ] {
            assert!(auto_start_eligible(kind, spec), "{kind:?} {spec}");
        }
        for (kind, spec) in [
            // Omitted bind: client GatewayPorts can widen it.
            (Local, "8080:web:80"),
            (Dynamic, "1080"),
            // Name-resolved bind: /etc/hosts can remap it.
            (Local, "localhost:8080:web:80"),
            (Dynamic, "localhost:1080"),
            // Non-loopback or wildcard binds.
            (Local, "0.0.0.0:8080:web:80"),
            (Local, "*:8080:web:80"),
            (Local, ":8080:web:80"),
            (Local, "127.0.0.2:8080:web:80"),
            (Local, "[::]:8080:web:80"),
            (Dynamic, "0.0.0.0:1080"),
            // Prefix-only tricks and non-TCP listeners.
            (Local, "127.0.0.1.evil:8080:web:80"),
            (Local, "127.0.0.1:0.0.0.0:8080:web:80"),
            (Local, "127.0.0.1:/tmp/listen.sock:web:80"),
            (Local, "127.0.0.1:8080"),
            (Local, "127.0.0.1:8080:"),
            (Local, "127.0.0.1:0:web:80"),
            (Local, "127.0.0.1:65536:web:80"),
            (Dynamic, "127.0.0.1:"),
            (Dynamic, "127.0.0.1:1080:x"),
            (Dynamic, "127.0.0.1:+1080"),
            // Remote: the server's GatewayPorts can override any bind.
            (Remote, "127.0.0.1:9000:127.0.0.1:3000"),
            (Remote, "[::1]:9000:[::1]:3000"),
            (Remote, "9000:127.0.0.1:3000"),
        ] {
            assert!(!auto_start_eligible(kind, spec), "{kind:?} {spec}");
        }
    }

    #[test]
    fn set_auto_start_refuses_ineligible_forwards_but_allows_opting_out() {
        let mut session = web_session();
        session.ssh.local_forwards.push("8080:web:80".into());
        session
            .ssh
            .local_forwards
            .push("localhost:8081:web:80".into());
        session
            .ssh
            .remote_forwards
            .push("127.0.0.1:9000:127.0.0.1:3000".into());
        session.ssh.dynamic_forwards.push("1080".into());
        let before = session.clone();
        for (kind, spec) in [
            (ForwardKind::Local, "8080:web:80"),
            (ForwardKind::Local, "localhost:8081:web:80"),
            (ForwardKind::Remote, "127.0.0.1:9000:127.0.0.1:3000"),
            (ForwardKind::Dynamic, "1080"),
        ] {
            assert!(
                set_auto_start(&mut session, kind, spec, true).is_err(),
                "{spec}"
            );
            assert_eq!(session, before);
            // Turning a (forged) opt-in off is always allowed.
            set_auto_start(&mut session, kind, spec, false).unwrap();
            assert_eq!(session, before);
        }
    }

    #[test]
    fn duplicate_metadata_uses_first_row_and_setting_collapses_it() {
        let mut session = web_session();
        for (label, auto_start) in [("First", false), ("Second", true)] {
            session.ssh.tunnel_labels.push(TunnelLabel {
                kind: TunnelKind::Local,
                spec: WEB.into(),
                label: label.into(),
                auto_start,
            });
        }
        assert!(!auto_start_for(&session, ForwardKind::Local, WEB));
        assert_eq!(label_for(&session, ForwardKind::Local, WEB), "First");
        set_auto_start(&mut session, ForwardKind::Local, WEB, true).unwrap();
        assert_eq!(session.ssh.tunnel_labels.len(), 1);
        assert_eq!(session.ssh.tunnel_labels[0].label, "First");
        assert!(auto_start_for(&session, ForwardKind::Local, WEB));
    }

    /// Forwarding-only `ssh -N` that stays alive without any server: the
    /// ProxyCommand never sends a banner, so no listener is ever bound.
    fn hung_ssh_config(dir: &Path) -> std::path::PathBuf {
        let config = dir.join("ssh_config");
        std::fs::write(&config, "Host *\n  ProxyCommand sleep 30\n").unwrap();
        config
    }

    #[test]
    fn remote_listener_conflicts_are_scoped_to_the_ssh_server() {
        let dir = tempfile::tempdir().unwrap();
        let config = hung_ssh_config(dir.path());
        let config = Some(config.as_path());
        let remote = "127.0.0.1:9000:127.0.0.1:3000";
        let local = "127.0.0.1:18080:web:80";
        let profile = |name: &str, host: &str| Session {
            name: name.into(),
            host: host.into(),
            ssh: crate::SshOptions {
                local_forwards: vec![local.into()],
                remote_forwards: vec![remote.into()],
                ..Default::default()
            },
            ..Default::default()
        };
        let one = profile("one", "one.invalid");
        let two = profile("two", "two.invalid");
        let one_again = profile("one again", "ONE.invalid");
        let mut manager = Manager::default();
        let (r, l) = (ForwardKind::Remote, ForwardKind::Local);

        let first = manager.start_forward(&one, r, remote, config).unwrap();
        assert!(first.is_running(), "{first:?}");
        // A distinct server has its own remote port namespace.
        let second = manager.start_forward(&two, r, remote, config).unwrap();
        assert!(second.is_running(), "distinct server: {second:?}");
        // Same server through another profile: that remote port is taken.
        let error = manager
            .start_forward(&one_again, r, remote, config)
            .unwrap_err();
        assert!(
            format!("{error:#}").contains("already managed"),
            "{error:#}"
        );
        // Local listeners share this machine whatever the server.
        assert!(
            manager
                .start_forward(&one, l, local, config)
                .unwrap()
                .is_running()
        );
        let error = manager.start_forward(&two, l, local, config).unwrap_err();
        assert!(
            format!("{error:#}").contains("already managed"),
            "{error:#}"
        );
        assert_eq!(manager.running_count(), 3);
        manager.stop_all();
        assert_eq!(manager.running_count(), 0);
    }

    /// `ssh -G` of OpenSSH 8.7 for a `Host charlie` stanza with
    /// LocalForward/RemoteForward/DynamicForward plus `-L`/`-D` flags.
    const MEASURED_EFFECTIVE: &str = "\
dynamicforward [127.0.0.1]:1081
dynamicforward 1080
localforward [127.0.0.1]:9100 [127.0.0.1]:7
localforward [::1]:9104 [::1]:7
localforward [127.0.0.1]:9101 [127.0.0.1]:7
remoteforward 9102 [localhost]:7
tunnel false
clearallforwardings no
";

    fn effective(local: &[&str], remote: &[&str], dynamic: &[&str]) -> EffectiveForwards {
        let owned = |values: &[&str]| values.iter().map(|value| value.to_string()).collect();
        EffectiveForwards {
            local: owned(local),
            remote: owned(remote),
            dynamic: owned(dynamic),
            tunnel_device: false,
        }
    }

    #[test]
    fn parse_exact_forward_effective_output_into_buckets() {
        let parsed = crate::terminal::parse_effective_forwards(MEASURED_EFFECTIVE);
        assert_eq!(
            parsed,
            effective(
                &[
                    "[127.0.0.1]:9100 [127.0.0.1]:7",
                    "[::1]:9104 [::1]:7",
                    "[127.0.0.1]:9101 [127.0.0.1]:7",
                ],
                &["9102 [localhost]:7"],
                &["[127.0.0.1]:1081", "1080"],
            )
        );
        assert!(!parsed.tunnel_device);
        let tun = crate::terminal::parse_effective_forwards("Tunnel point-to-point\n");
        assert!(tun.tunnel_device);
        let off = crate::terminal::parse_effective_forwards("TUNNEL no\n");
        assert!(!off.tunnel_device);
        assert_eq!(
            crate::terminal::parse_effective_forwards("LocalForward 9 [h]:1\n").local,
            vec!["9 [h]:1".to_owned()]
        );
    }

    #[test]
    fn ensure_exact_forward_accepts_only_selected_forward() {
        let (l, d) = (ForwardKind::Local, ForwardKind::Dynamic);
        for (kind, spec, effective) in [
            (
                l,
                "127.0.0.1:9100:127.0.0.1:7",
                effective(&["[127.0.0.1]:9100 [127.0.0.1]:7"], &[], &[]),
            ),
            (
                l,
                "[::1]:9104:[::1]:7",
                effective(&["[::1]:9104 [::1]:7"], &[], &[]),
            ),
            (
                l,
                "127.0.0.1:8080:web:80",
                effective(&["[127.0.0.1]:8080 [web]:80"], &[], &[]),
            ),
            (
                d,
                "127.0.0.1:1081",
                effective(&[], &[], &["[127.0.0.1]:1081"]),
            ),
        ] {
            ensure_exact_forward(&effective, kind, spec).unwrap_or_else(|error| {
                panic!("{spec} must be accepted: {error:#}");
            });
        }
    }

    #[test]
    fn ensure_exact_forward_rejects_inherited_or_changed_forwards() {
        let (l, d) = (ForwardKind::Local, ForwardKind::Dynamic);
        let web = "127.0.0.1:9100:127.0.0.1:7";
        let web_effective = "[127.0.0.1]:9100 [127.0.0.1]:7";
        let mut tunnel = effective(&[web_effective], &[], &[]);
        tunnel.tunnel_device = true;
        for (case, kind, spec, effective) in [
            (
                "extra inherited loopback LocalForward",
                l,
                web,
                effective(&[web_effective, "[127.0.0.1]:9101 [127.0.0.1]:7"], &[], &[]),
            ),
            (
                "inherited RemoteForward",
                l,
                web,
                effective(&[web_effective], &["9102 [localhost]:7"], &[]),
            ),
            (
                "inherited DynamicForward",
                l,
                web,
                effective(&[web_effective], &[], &["1080"]),
            ),
            (
                "inherited wildcard LocalForward",
                l,
                web,
                effective(&[web_effective, "[0.0.0.0]:9200 [web]:80"], &[], &[]),
            ),
            (
                "inherited star LocalForward",
                l,
                web,
                effective(&[web_effective, "[*]:9201 [web]:80"], &[], &[]),
            ),
            (
                "ClearAllForwardings leaves nothing",
                l,
                web,
                effective(&[], &[], &[]),
            ),
            (
                "selected listener with a different target",
                l,
                web,
                effective(&["[127.0.0.1]:9100 [127.0.0.1]:8"], &[], &[]),
            ),
            (
                "selected listener widened by config",
                l,
                web,
                effective(&["9100 [127.0.0.1]:7"], &[], &[]),
            ),
            (
                "same listener under another kind",
                d,
                "127.0.0.1:9100",
                effective(&[web_effective], &[], &[]),
            ),
            (
                "unparseable effective value",
                l,
                web,
                effective(&["/tmp/socket /tmp/target"], &[], &[]),
            ),
            ("tunnel device", l, web, tunnel),
        ] {
            let error = ensure_exact_forward(&effective, kind, spec)
                .expect_err(case)
                .to_string();
            assert_eq!(error, InheritedForwards.to_string(), "{case}");
        }
    }

    #[test]
    fn exact_forward_preflight_rejects_inherited_forward_before_spawning() {
        let dir = tempfile::tempdir().unwrap();
        let config = hung_ssh_config(dir.path());
        let spec = "127.0.0.1:18090:web:80";
        let session = Session {
            name: "srv".into(),
            host: "srv.invalid".into(),
            ssh: crate::SshOptions {
                local_forwards: vec![spec.into(), "127.0.0.1:18091:db:5432".into()],
                ..Default::default()
            },
            ..Default::default()
        };
        let asked = std::cell::Cell::new(0);
        let fake: Preflight<'_> = &|selected, _| {
            asked.set(asked.get() + 1);
            // The preflight sees the isolated launch, not the whole profile.
            assert_eq!(selected.ssh.local_forwards, vec![spec.to_owned()]);
            Ok(effective(
                &[
                    "[127.0.0.1]:18090 [web]:80",
                    "[127.0.0.1]:18092 [127.0.0.1]:7",
                ],
                &[],
                &[],
            ))
        };
        let mut manager = Manager::default();
        // Reaching start_tunnels would yield a running (hung) ssh here.
        let result = manager.launch_forward(
            &session,
            ForwardKind::Local,
            spec,
            Some(&config),
            Some(fake),
        );
        assert_eq!(asked.get(), 1);
        let error = result.expect_err("inherited forward must refuse the launch");
        assert!(
            error.downcast_ref::<InheritedForwards>().is_some(),
            "{error:#}"
        );
        let key = session_profile_key(&session);
        assert_eq!(
            manager.forward_status(&key, ForwardKind::Local, spec),
            Status::Stopped
        );
        assert_eq!(manager.running_count(), 0);
        assert!(manager.processes.is_empty());
        assert!(manager.failures.is_empty());
    }

    #[test]
    fn exact_forward_preflight_launches_when_only_selected_forward() {
        let dir = tempfile::tempdir().unwrap();
        let config = hung_ssh_config(dir.path());
        let spec = "127.0.0.1:18093:web:80";
        let session = Session {
            name: "srv".into(),
            host: "srv.invalid".into(),
            ssh: crate::SshOptions {
                local_forwards: vec![spec.into()],
                ..Default::default()
            },
            ..Default::default()
        };
        let fake: Preflight<'_> = &|_, _| Ok(effective(&["[127.0.0.1]:18093 [web]:80"], &[], &[]));
        let mut manager = Manager::default();
        let status = manager
            .launch_forward(
                &session,
                ForwardKind::Local,
                spec,
                Some(&config),
                Some(fake),
            )
            .unwrap();
        assert!(status.is_running(), "{status:?}");
        manager.stop_all();
    }

    #[test]
    fn manual_start_skips_exact_forward_check() {
        // A config that adds a forward does not change manual semantics.
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("ssh_config");
        std::fs::write(
            &config,
            "Host *\n  ProxyCommand sleep 30\n  LocalForward 127.0.0.1:18095 127.0.0.1:7\n",
        )
        .unwrap();
        let spec = "127.0.0.1:18094:web:80";
        let session = Session {
            name: "srv".into(),
            host: "srv.invalid".into(),
            ssh: crate::SshOptions {
                local_forwards: vec![spec.into()],
                ..Default::default()
            },
            ..Default::default()
        };
        let mut manager = Manager::default();
        let status = manager
            .start_forward(&session, ForwardKind::Local, spec, Some(&config))
            .unwrap();
        assert!(status.is_running(), "{status:?}");
        manager.stop_all();
    }

    /// Real OpenSSH, no network: `ssh -G` only evaluates the configuration.
    #[test]
    fn real_openssh_exact_forward_rejects_config_inherited_forward() {
        let dir = tempfile::tempdir().unwrap();
        let spec = "127.0.0.1:18096:127.0.0.1:7";
        let session = Session {
            name: "srv".into(),
            host: "exactfwd.invalid".into(),
            ssh: crate::SshOptions {
                local_forwards: vec![spec.into()],
                ..Default::default()
            },
            ..Default::default()
        };
        let inherited = dir.path().join("inherited_config");
        std::fs::write(
            &inherited,
            "Host exactfwd.invalid\n  ServerAliveInterval 30\n  \
LocalForward 127.0.0.1:18097 127.0.0.1:7\n",
        )
        .unwrap();
        let plain = dir.path().join("plain_config");
        std::fs::write(&plain, "Host exactfwd.invalid\n  ServerAliveInterval 30\n").unwrap();

        let found = crate::terminal::effective_forwards(&session, Some(&inherited)).unwrap();
        assert_eq!(found.local.len(), 2, "{found:?}");
        let error = ensure_exact_forward(&found, ForwardKind::Local, spec).unwrap_err();
        assert!(
            error.downcast_ref::<InheritedForwards>().is_some(),
            "{error:#}"
        );

        let found = crate::terminal::effective_forwards(&session, Some(&plain)).unwrap();
        ensure_exact_forward(&found, ForwardKind::Local, spec).unwrap();
    }
}
