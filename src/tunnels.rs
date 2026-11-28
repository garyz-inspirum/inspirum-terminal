//! Centralised SSH tunnel manager model.
//!
//! Forwarding-only OpenSSH processes can own either a whole saved profile or
//! one saved forward. The manager tracks their lifetimes independently.
//! It deliberately contains no Iced types so it can be unit tested.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Result, ensure};

use crate::terminal::{
    EffectiveForwards, FrozenConfig, TunnelConfig, TunnelProcess, frozen_config_text,
    parse_effective_forwards, tunnel_launch_args, tunnel_launch_args_frozen,
};
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
        let seam = Seam {
            exact: None,
            spawn: &crate::terminal::spawn_tunnel,
        };
        self.launch_forward(session, kind, spec, config, seam)
    }

    /// [`Self::start_forward`] for unattended launches. Ask OpenSSH
    /// (`ssh -G`) what the launch would set up, refuse ProxyJump and anything
    /// but exactly the selected forward, then freeze that one evaluation into
    /// a private configuration without `Host`/`Match`/`Include`, re-check it
    /// and spawn ssh reading only the frozen file, so a second evaluation by
    /// the real process cannot add forwards.
    pub fn start_forward_exact(
        &mut self,
        session: &Session,
        kind: ForwardKind,
        spec: &str,
        config: Option<&Path>,
    ) -> Result<Status> {
        let frozen_dir = std::env::temp_dir();
        let seam = Seam {
            exact: Some(ExactCheck {
                query: &crate::terminal::query_effective_config,
                frozen_dir: &frozen_dir,
            }),
            spawn: &crate::terminal::spawn_tunnel,
        };
        self.launch_forward(session, kind, spec, config, seam)
    }

    /// Shared body of the start variants. `seam.exact` runs strictly before
    /// `seam.spawn`; its failure returns `Err` with nothing spawned or
    /// recorded.
    fn launch_forward(
        &mut self,
        session: &Session,
        kind: ForwardKind,
        spec: &str,
        config: Option<&Path>,
        seam: Seam<'_>,
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
        let launch_config = match &seam.exact {
            None => TunnelConfig::User(config),
            Some(exact) => TunnelConfig::Frozen(exact.freeze(&selected, kind, spec, config)?),
        };
        self.failures.remove(&key);
        match (seam.spawn)(&selected, launch_config) {
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

/// How [`Manager::launch_forward`] checks and spawns; tests inject fakes.
struct Seam<'a> {
    /// `None` for manual starts: no `ssh -G`, the user configuration as is.
    exact: Option<ExactCheck<'a>>,
    spawn: &'a dyn Fn(&Session, TunnelConfig<'_>) -> Result<TunnelProcess>,
}

/// Unattended-launch checks of [`Manager::start_forward_exact`].
struct ExactCheck<'a> {
    /// `ssh -G` stdout for a tunnel argv.
    query: &'a dyn Fn(&[String]) -> Result<String>,
    frozen_dir: &'a Path,
}

impl ExactCheck<'_> {
    /// Validate `selected` as launched with `config`, freeze that
    /// evaluation, and validate the frozen launch again.
    fn freeze(
        &self,
        selected: &Session,
        kind: ForwardKind,
        spec: &str,
        config: Option<&Path>,
    ) -> Result<FrozenConfig> {
        let resolved = (self.query)(&tunnel_launch_args(selected, config)?)?;
        ensure_unattended(&parse_effective_forwards(&resolved), kind, spec)?;
        let frozen = FrozenConfig::write(self.frozen_dir, &frozen_config_text(&resolved)?)?;
        let relaunch = (self.query)(&tunnel_launch_args_frozen(selected, frozen.path())?)?;
        ensure_unattended(&parse_effective_forwards(&relaunch), kind, spec)?;
        Ok(frozen)
    }
}

/// The frozen configuration describes the destination, not a jump host:
/// losing the hop's own settings could change where SSH connects.
fn ensure_unattended(effective: &EffectiveForwards, kind: ForwardKind, spec: &str) -> Result<()> {
    if effective.proxy_jump.is_some() {
        return Err(ProxyJumpSkipped.into());
    }
    ensure_exact_forward(effective, kind, spec)
}

/// An unattended launch would go through an effective `ProxyJump`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProxyJumpSkipped;

impl std::fmt::Display for ProxyJumpSkipped {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("Auto-start skipped: this tunnel uses ProxyJump. Start it manually.")
    }
}

impl std::error::Error for ProxyJumpSkipped {}

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

/// The OpenSSH configuration an unattended launch would read cannot be
/// captured as one private, self-contained snapshot, so the forward is
/// skipped at start-up and left for a manual start.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConfigSkip {
    /// The app runs without `--ssh-config`: OpenSSH would read files the app
    /// cannot snapshot without evaluating them first.
    NoExplicitConfig,
    /// The configuration has an `Include` or `Match` line.
    IncludeOrMatch,
    /// The configuration file is missing, unreadable or not UTF-8.
    Unreadable,
    /// `CanonicalizeHostname` re-evaluates `Host` blocks against DNS, so the
    /// preflight and the launch could see different settings.
    CanonicalizeHostname,
}

impl std::fmt::Display for ConfigSkip {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::NoExplicitConfig => {
                "Auto-start skipped: no explicit SSH configuration was provided. \
Start this tunnel manually."
            }
            Self::IncludeOrMatch => {
                "Auto-start skipped: this tunnel's OpenSSH configuration uses Include or \
Match. Start it manually."
            }
            Self::Unreadable => {
                "Auto-start skipped: the SSH configuration file could not be read as UTF-8 \
text. Start it manually."
            }
            Self::CanonicalizeHostname => {
                "Auto-start skipped: this tunnel's OpenSSH configuration uses \
CanonicalizeHostname. Start it manually."
            }
        })
    }
}

impl std::error::Error for ConfigSkip {}

/// Ok only when `effective` (from `ssh -G`) is exactly the selected forward.
pub fn ensure_exact_forward(
    effective: &EffectiveForwards,
    kind: ForwardKind,
    spec: &str,
) -> Result<()> {
    let total = effective.local.len() + effective.remote.len() + effective.dynamic.len();
    let only = [
        (ForwardKind::Local, &effective.local),
        (ForwardKind::Remote, &effective.remote),
        (ForwardKind::Dynamic, &effective.dynamic),
    ]
    .into_iter()
    .find_map(|(found, values)| values.first().map(|value| (found, value)));
    let exact = !effective.tunnel_device
        && total == 1
        && only.is_some_and(|(found, value)| {
            found == kind
                && matches!(
                    (effective_endpoints(kind, value), spec_endpoints(kind, spec)),
                    (Some(actual), Some(wanted)) if actual == wanted
                )
        });
    if exact {
        Ok(())
    } else {
        Err(InheritedForwards.into())
    }
}

/// A forward side normalised for comparison: brackets stripped, the host
/// absent when only a port is given.
#[derive(Debug, PartialEq, Eq)]
struct Endpoint {
    host: Option<String>,
    port: u16,
}

/// Listener and (for Local/Remote) target of one forward.
type Endpoints = (Endpoint, Option<Endpoint>);

/// `[host]:port`, `host:port` or a bare `port`; anything else (socket
/// paths, empty or unbalanced hosts, port 0) is `None` so callers fail closed.
fn parse_endpoint(text: &str) -> Option<Endpoint> {
    let (host, port) = if let Some(rest) = text.strip_prefix('[') {
        let (host, port) = rest.split_once("]:")?;
        (Some(host), port)
    } else if let Some((host, port)) = text.rsplit_once(':') {
        if host.contains(':') {
            return None;
        }
        (Some(host), port)
    } else {
        (None, text)
    };
    if host.is_some_and(|host| {
        host.is_empty() || host.contains(['[', ']', '/']) || host.chars().any(char::is_whitespace)
    }) {
        return None;
    }
    if port.is_empty() || port.len() > 5 || !port.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let port = port.parse::<u16>().ok().filter(|port| *port != 0)?;
    Some(Endpoint {
        host: host.map(str::to_owned),
        port,
    })
}

fn target_endpoint(text: &str) -> Option<Endpoint> {
    parse_endpoint(text).filter(|target| target.host.is_some())
}

/// One `ssh -G` forward value: `listener target` (Local/Remote) or `listener`.
fn effective_endpoints(kind: ForwardKind, value: &str) -> Option<Endpoints> {
    let mut parts = value.split_whitespace();
    let listener = parse_endpoint(parts.next()?)?;
    let target = match kind {
        ForwardKind::Dynamic => None,
        _ => Some(target_endpoint(parts.next()?)?),
    };
    parts.next().is_none().then_some((listener, target))
}

/// A saved forward spec in OpenSSH `-L`/`-R`/`-D` syntax.
fn spec_endpoints(kind: ForwardKind, spec: &str) -> Option<Endpoints> {
    if spec.chars().any(char::is_whitespace) {
        return None;
    }
    let (listener, target) = split_spec(kind, spec);
    let listener = parse_endpoint(&listener)?;
    let target = match kind {
        ForwardKind::Dynamic => None,
        _ => Some(target_endpoint(&target)?),
    };
    Some((listener, target))
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
            proxy_jump: None,
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

    fn single_forward_session(host: &str, specs: &[&str]) -> Session {
        Session {
            name: "srv".into(),
            host: host.into(),
            ssh: crate::SshOptions {
                local_forwards: specs.iter().map(|spec| spec.to_string()).collect(),
                ..Default::default()
            },
            ..Default::default()
        }
    }

    fn dir_entries(dir: &Path) -> Vec<String> {
        let mut names: Vec<_> = std::fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    fn config_arg(args: &[String]) -> Option<&str> {
        args.windows(2)
            .find(|pair| pair[0] == "-F")
            .map(|pair| pair[1].as_str())
    }

    /// What the fake spawn of [`exact_forward_attempt`] saw.
    #[derive(Debug)]
    struct SeenSpawn {
        args: Vec<String>,
        /// Bytes of the `-F` file at spawn time.
        config_bytes: Vec<u8>,
        /// The spawn read the user configuration (manual semantics).
        user: bool,
        /// Real `ssh -G` of the spawn argv (no network), when it succeeds.
        resolved: Option<String>,
    }

    /// One unattended launch through the exact seam with `query` and a spawn
    /// that records what it would launch and never starts ssh.
    fn exact_forward_attempt(
        session: &Session,
        spec: &str,
        config: Option<&Path>,
        frozen_dir: &Path,
        query: &dyn Fn(&[String]) -> Result<String>,
    ) -> (Result<Status>, Vec<SeenSpawn>, Manager) {
        let seen = std::cell::RefCell::new(Vec::new());
        let spawn = |selected: &Session, launch: TunnelConfig<'_>| -> Result<TunnelProcess> {
            let args = launch.launch_args(selected)?;
            let config_bytes = launch
                .path()
                .map(|path| std::fs::read(path).unwrap())
                .unwrap_or_default();
            let resolved = crate::terminal::query_effective_config(&args).ok();
            seen.borrow_mut().push(SeenSpawn {
                user: matches!(launch, TunnelConfig::User(_)),
                args,
                config_bytes,
                resolved,
            });
            anyhow::bail!("fake spawn")
        };
        let mut manager = Manager::default();
        let seam = Seam {
            exact: Some(ExactCheck { query, frozen_dir }),
            spawn: &spawn,
        };
        let result = manager.launch_forward(session, ForwardKind::Local, spec, config, seam);
        (result, seen.into_inner(), manager)
    }

    /// The launch was skipped for `want` with nothing spawned, recorded or
    /// left in `frozen_dir`.
    fn assert_exact_forward_skipped(
        result: Result<Status>,
        seen: &[SeenSpawn],
        manager: &Manager,
        frozen_dir: &Path,
        want: ConfigSkip,
    ) {
        let error = result.expect_err("the unattended launch must be skipped");
        assert_eq!(error.downcast_ref::<ConfigSkip>(), Some(&want), "{error:#}");
        assert_eq!(error.to_string(), want.to_string());
        assert!(seen.is_empty(), "spawn reached: {seen:?}");
        assert!(manager.processes.is_empty());
        assert!(manager.failures.is_empty());
        assert!(dir_entries(frozen_dir).is_empty());
    }

    /// The `keyword` line real `ssh -G` prints for a tunnel argv reading
    /// `config` (empty when absent).
    fn exact_forward_printed(config: &Path, keyword: &str) -> String {
        let session = single_forward_session("exactfwd.invalid", &["127.0.0.1:18110:127.0.0.1:7"]);
        let output = crate::terminal::query_effective_config(
            &tunnel_launch_args(&session, Some(config)).unwrap(),
        )
        .unwrap();
        printed_line(&output, keyword)
    }

    fn printed_line(output: &str, keyword: &str) -> String {
        output
            .lines()
            .find(|line| line.split_whitespace().next() == Some(keyword))
            .unwrap_or_default()
            .to_owned()
    }

    /// A fake `ssh -G` that counts its calls and reports only `spec`'s
    /// listener 127.0.0.1:18111 -> 127.0.0.1:7.
    fn counting_query(count: &std::cell::Cell<usize>) -> impl Fn(&[String]) -> Result<String> + '_ {
        move |_: &[String]| {
            count.set(count.get() + 1);
            Ok("localforward [127.0.0.1]:18111 [127.0.0.1]:7\ntunnel false\n".into())
        }
    }

    const COUNTED_SPEC: &str = "127.0.0.1:18111:127.0.0.1:7";

    /// THE PAIR, first half: a direct HostName/User/Port config starts, from
    /// a private snapshot holding exactly the source bytes.
    #[test]
    fn real_openssh_exact_forward_snapshot_starts_defaults_only_config() {
        let dir = tempfile::tempdir().unwrap();
        let frozen_dir = tempfile::tempdir().unwrap();
        let spec = "127.0.0.1:18110:127.0.0.1:7";
        let session = single_forward_session("exactfwd.invalid", &[spec]);
        let source = dir.path().join("ssh_config");
        let bytes = b"Host exactfwd.invalid\n  HostName 127.0.0.1\n  User bob\n  Port 2222\n";
        std::fs::write(&source, bytes).unwrap();
        let (result, seen, _manager) = exact_forward_attempt(
            &session,
            spec,
            Some(&source),
            frozen_dir.path(),
            &crate::terminal::query_effective_config,
        );
        assert_eq!(result.unwrap(), Status::Failed("fake spawn".into()));
        let [spawn] = seen.as_slice() else {
            panic!("one spawn expected: {seen:?}");
        };
        assert!(!spawn.user);
        let snapshot = config_arg(&spawn.args).expect("snapshot -F");
        assert!(snapshot.starts_with(frozen_dir.path().to_str().unwrap()));
        assert!(
            !spawn.args.iter().any(|arg| arg == source.to_str().unwrap()),
            "{:?}",
            spawn.args
        );
        assert_eq!(
            String::from_utf8_lossy(&spawn.config_bytes),
            String::from_utf8_lossy(bytes)
        );
        // The snapshot lives only as long as the (failed) process.
        assert!(dir_entries(frozen_dir.path()).is_empty());
    }

    /// THE PAIR, second half: ONE quoted known-hosts path whose `ssh -G`
    /// text equals the two separate defaults is preserved as written.
    #[test]
    fn real_openssh_exact_forward_snapshot_preserves_quoted_default_known_hosts() {
        let dir = tempfile::tempdir().unwrap();
        let frozen_dir = tempfile::tempdir().unwrap();
        let spec = "127.0.0.1:18112:127.0.0.1:7";
        let session = single_forward_session("exactfwd.invalid", &[spec]);
        let empty = dir.path().join("empty_config");
        std::fs::write(&empty, b"").unwrap();
        let default_line = exact_forward_printed(&empty, "userknownhostsfile");
        let default_value = default_line
            .strip_prefix("userknownhostsfile ")
            .expect("ssh -G prints userknownhostsfile");
        let plain = dir.path().join("plain_config");
        let direct = "Host exactfwd.invalid\n  HostName 127.0.0.1\n  User bob\n  Port 2222\n";
        std::fs::write(&plain, direct).unwrap();
        let source = dir.path().join("ssh_config");
        let bytes = format!("{direct}  UserKnownHostsFile \"{default_value}\"\n");
        std::fs::write(&source, &bytes).unwrap();
        // The fixture is the indistinguishable case: one quoted path prints
        // exactly like the separate defaults.
        assert_eq!(
            exact_forward_printed(&source, "userknownhostsfile"),
            exact_forward_printed(&plain, "userknownhostsfile")
        );
        let (result, seen, _manager) = exact_forward_attempt(
            &session,
            spec,
            Some(&source),
            frozen_dir.path(),
            &crate::terminal::query_effective_config,
        );
        assert_eq!(result.unwrap(), Status::Failed("fake spawn".into()));
        let [spawn] = seen.as_slice() else {
            panic!("one spawn expected: {seen:?}");
        };
        assert_eq!(String::from_utf8_lossy(&spawn.config_bytes), bytes);
        let resolved = spawn
            .resolved
            .as_deref()
            .expect("ssh -G of the launch argv");
        assert_eq!(
            printed_line(resolved, "userknownhostsfile"),
            exact_forward_printed(&source, "userknownhostsfile")
        );
    }

    /// The reviewer's case: a quoted known-hosts path with a space and `#`.
    #[test]
    fn real_openssh_exact_forward_snapshot_preserves_known_hosts_with_hash() {
        let dir = tempfile::tempdir().unwrap();
        let frozen_dir = tempfile::tempdir().unwrap();
        let spec = "127.0.0.1:18113:127.0.0.1:7";
        let session = single_forward_session("exactfwd.invalid", &[spec]);
        let source = dir.path().join("ssh_config");
        let bytes = "Host exactfwd.invalid\n  UserKnownHostsFile \"/example/host keys #1\"\n";
        std::fs::write(&source, bytes).unwrap();
        let original = exact_forward_printed(&source, "userknownhostsfile");
        assert_eq!(original, "userknownhostsfile /example/host keys #1");
        let (result, seen, _manager) = exact_forward_attempt(
            &session,
            spec,
            Some(&source),
            frozen_dir.path(),
            &crate::terminal::query_effective_config,
        );
        assert_eq!(result.unwrap(), Status::Failed("fake spawn".into()));
        let [spawn] = seen.as_slice() else {
            panic!("one spawn expected: {seen:?}");
        };
        assert_eq!(String::from_utf8_lossy(&spawn.config_bytes), bytes);
        let resolved = spawn
            .resolved
            .as_deref()
            .expect("ssh -G of the launch argv");
        assert_eq!(printed_line(resolved, "userknownhostsfile"), original);
    }

    #[test]
    fn exact_forward_snapshot_rejects_include_or_match() {
        let dir = tempfile::tempdir().unwrap();
        let frozen_dir = tempfile::tempdir().unwrap();
        let session = single_forward_session("exactfwd.invalid", &[COUNTED_SPEC]);
        let source = dir.path().join("ssh_config");
        for text in [
            "Include other.conf\n",
            "Host x\n  Match host x\n",
            "  include=foo\n",
            "MATCH all\n",
            "Host *\n\tMatch exec true\n",
            "\"Include\" other.conf\n",
            "Host exactfwd.invalid\r\n  Include=other.conf\r\n",
        ] {
            std::fs::write(&source, text).unwrap();
            let asked = std::cell::Cell::new(0);
            let query = counting_query(&asked);
            let (result, seen, manager) = exact_forward_attempt(
                &session,
                COUNTED_SPEC,
                Some(&source),
                frozen_dir.path(),
                &query,
            );
            assert_exact_forward_skipped(
                result,
                &seen,
                &manager,
                frozen_dir.path(),
                ConfigSkip::IncludeOrMatch,
            );
            assert_eq!(asked.get(), 0, "{text:?}");
        }
        // A comment and a value that merely contains the word are not
        // `Include`/`Match` lines.
        let text = "# Include x\nHost exactfwd.invalid\n  HostName include.example\n";
        std::fs::write(&source, text).unwrap();
        let asked = std::cell::Cell::new(0);
        let query = counting_query(&asked);
        let (result, seen, _manager) = exact_forward_attempt(
            &session,
            COUNTED_SPEC,
            Some(&source),
            frozen_dir.path(),
            &query,
        );
        assert_eq!(result.unwrap(), Status::Failed("fake spawn".into()));
        assert_eq!(asked.get(), 1);
        let [spawn] = seen.as_slice() else {
            panic!("one spawn expected: {seen:?}");
        };
        assert_eq!(String::from_utf8_lossy(&spawn.config_bytes), text);
    }

    #[test]
    fn exact_forward_snapshot_skips_missing_or_non_utf8_config() {
        let dir = tempfile::tempdir().unwrap();
        let frozen_dir = tempfile::tempdir().unwrap();
        let session = single_forward_session("exactfwd.invalid", &[COUNTED_SPEC]);
        let missing = dir.path().join("does_not_exist");
        let binary = dir.path().join("binary_config");
        std::fs::write(&binary, b"Host exactfwd.invalid\n  User \xff\n").unwrap();
        for source in [&missing, &binary] {
            let asked = std::cell::Cell::new(0);
            let query = counting_query(&asked);
            let (result, seen, manager) = exact_forward_attempt(
                &session,
                COUNTED_SPEC,
                Some(source),
                frozen_dir.path(),
                &query,
            );
            assert_exact_forward_skipped(
                result,
                &seen,
                &manager,
                frozen_dir.path(),
                ConfigSkip::Unreadable,
            );
            assert_eq!(asked.get(), 0, "{source:?}");
        }
    }

    /// Without `--ssh-config` nothing runs: no `ssh -G`, no snapshot, no
    /// spawn.
    #[test]
    fn exact_forward_without_explicit_config_skips_before_any_ssh() {
        let frozen_dir = tempfile::tempdir().unwrap();
        let session = single_forward_session("exactfwd.invalid", &[COUNTED_SPEC]);
        let asked = std::cell::Cell::new(0);
        let query = counting_query(&asked);
        let (result, seen, manager) =
            exact_forward_attempt(&session, COUNTED_SPEC, None, frozen_dir.path(), &query);
        assert_exact_forward_skipped(
            result,
            &seen,
            &manager,
            frozen_dir.path(),
            ConfigSkip::NoExplicitConfig,
        );
        assert_eq!(asked.get(), 0);
    }

    #[test]
    fn real_openssh_exact_forward_skips_canonicalize_hostname() {
        let dir = tempfile::tempdir().unwrap();
        let frozen_dir = tempfile::tempdir().unwrap();
        let spec = "127.0.0.1:18114:127.0.0.1:7";
        let session = single_forward_session("exactfwd.invalid", &[spec]);
        let source = dir.path().join("ssh_config");
        std::fs::write(
            &source,
            "Host exactfwd.invalid\n  CanonicalizeHostname yes\n",
        )
        .unwrap();
        let (result, seen, manager) = exact_forward_attempt(
            &session,
            spec,
            Some(&source),
            frozen_dir.path(),
            &crate::terminal::query_effective_config,
        );
        assert_exact_forward_skipped(
            result,
            &seen,
            &manager,
            frozen_dir.path(),
            ConfigSkip::CanonicalizeHostname,
        );
    }

    /// Manual start reads the user configuration as is, even one with
    /// `Include` and `Match`, and runs no query.
    #[test]
    fn manual_start_with_include_or_match_skips_exact_forward_check() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("ssh_config");
        std::fs::write(&source, "Include other.conf\nMatch all\n  User bob\n").unwrap();
        let session = single_forward_session("exactfwd.invalid", &[COUNTED_SPEC]);
        let seen = std::cell::RefCell::new(Vec::new());
        let spawn = |selected: &Session, launch: TunnelConfig<'_>| -> Result<TunnelProcess> {
            assert!(matches!(launch, TunnelConfig::User(_)));
            seen.borrow_mut().push(launch.launch_args(selected)?);
            anyhow::bail!("fake spawn")
        };
        let seam = Seam {
            exact: None,
            spawn: &spawn,
        };
        let mut manager = Manager::default();
        let status = manager
            .launch_forward(
                &session,
                ForwardKind::Local,
                COUNTED_SPEC,
                Some(&source),
                seam,
            )
            .unwrap();
        assert_eq!(status, Status::Failed("fake spawn".into()));
        let seen = seen.into_inner();
        let [args] = seen.as_slice() else {
            panic!("one spawn expected: {seen:?}");
        };
        assert_eq!(config_arg(args), source.to_str());
    }

    #[test]
    fn exact_forward_preflight_rejects_inherited_forward_before_spawning() {
        let dir = tempfile::tempdir().unwrap();
        let frozen_dir = tempfile::tempdir().unwrap();
        let config = hung_ssh_config(dir.path());
        let spec = "127.0.0.1:18090:web:80";
        let session = single_forward_session("srv.invalid", &[spec, "127.0.0.1:18091:db:5432"]);
        let asked = std::cell::Cell::new(0);
        let spawned = std::cell::Cell::new(false);
        let query = |args: &[String]| -> Result<String> {
            asked.set(asked.get() + 1);
            // The query sees the isolated launch, not the whole profile.
            assert!(args.iter().any(|arg| arg == spec));
            assert!(!args.iter().any(|arg| arg == "127.0.0.1:18091:db:5432"));
            Ok("localforward [127.0.0.1]:18090 [web]:80\n\
localforward [127.0.0.1]:18092 [127.0.0.1]:7\ntunnel false\n"
                .into())
        };
        let spawn = |_: &Session, _: TunnelConfig<'_>| -> Result<TunnelProcess> {
            spawned.set(true);
            anyhow::bail!("spawn must not be reached")
        };
        let seam = Seam {
            exact: Some(ExactCheck {
                query: &query,
                frozen_dir: frozen_dir.path(),
            }),
            spawn: &spawn,
        };
        let mut manager = Manager::default();
        let result =
            manager.launch_forward(&session, ForwardKind::Local, spec, Some(&config), seam);
        assert_eq!(asked.get(), 1);
        assert!(!spawned.get());
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
        assert!(dir_entries(frozen_dir.path()).is_empty());
    }

    /// The spawn reads only the frozen file, never the user configuration,
    /// and the frozen file lives exactly as long as the process.
    #[test]
    fn exact_forward_spawn_uses_frozen_launch_argv() {
        let dir = tempfile::tempdir().unwrap();
        let frozen_dir = tempfile::tempdir().unwrap();
        let config = hung_ssh_config(dir.path());
        let user_config = config.to_str().unwrap().to_owned();
        let spec = "127.0.0.1:18093:web:80";
        let session = single_forward_session("srv.invalid", &[spec]);
        let queried = std::cell::RefCell::new(Vec::new());
        let spawned = std::cell::RefCell::new(Vec::new());
        let query = |args: &[String]| -> Result<String> {
            queried.borrow_mut().push(args.to_vec());
            Ok(
                "localforward [127.0.0.1]:18093 [web]:80\nproxycommand sleep 30\ntunnel false\n"
                    .into(),
            )
        };
        let spawn = |selected: &Session, launch: TunnelConfig<'_>| -> Result<TunnelProcess> {
            let args = launch.launch_args(selected)?;
            // The snapshot holds the user's bytes unchanged.
            let snapshot = std::fs::read_to_string(launch.path().unwrap()).unwrap();
            assert_eq!(snapshot, "Host *\n  ProxyCommand sleep 30\n");
            spawned.borrow_mut().push(args);
            crate::terminal::spawn_tunnel(selected, launch)
        };
        let seam = Seam {
            exact: Some(ExactCheck {
                query: &query,
                frozen_dir: frozen_dir.path(),
            }),
            spawn: &spawn,
        };
        let mut manager = Manager::default();
        let status = manager
            .launch_forward(&session, ForwardKind::Local, spec, Some(&config), seam)
            .unwrap();
        assert!(status.is_running(), "{status:?}");

        let queried = queried.into_inner();
        let spawned = spawned.into_inner();
        // One preflight, on the snapshot, never on the user configuration.
        assert_eq!(queried.len(), 1);
        assert!(!queried[0].iter().any(|arg| arg == &user_config));
        let [args] = spawned.as_slice() else {
            panic!("one spawn expected: {spawned:?}");
        };
        let frozen = config_arg(args).expect("frozen -F").to_owned();
        assert!(frozen.starts_with(frozen_dir.path().to_str().unwrap()));
        assert!(!args.iter().any(|arg| arg == &user_config), "{args:?}");
        assert_eq!(args.iter().filter(|arg| *arg == "-F").count(), 1);
        assert!(
            args.windows(2)
                .any(|pair| pair[0] == "-L" && pair[1] == spec)
        );
        // The preflight ran on exactly the argv that was spawned.
        assert_eq!(&queried[0], args);
        assert_eq!(dir_entries(frozen_dir.path()).len(), 1);
        manager.stop_all();
        assert!(dir_entries(frozen_dir.path()).is_empty());
    }

    #[test]
    fn exact_forward_frozen_config_drops_forwards_and_rejects_scoping() {
        let frozen = crate::terminal::frozen_config_text(
            "user bob\nhostname 10.0.0.1\nLocalForward [127.0.0.1]:1 [h]:2\n\
remoteforward 9102 [localhost]:7\ndynamicforward 1080\n\nproxycommand sh -c 'nc %h %p'\n",
        )
        .unwrap();
        assert_eq!(
            frozen,
            "user bob\nhostname 10.0.0.1\nproxycommand sh -c 'nc %h %p'\n"
        );
        // OpenSSH 9.x prints the destination as an informational first line.
        assert_eq!(
            crate::terminal::frozen_config_text("host exactfwd.invalid\nuser bob\n").unwrap(),
            "user bob\n"
        );
        for line in [
            "Host *",
            "host exactfwd.invalid",
            "match all",
            "Include other.conf",
        ] {
            let output = format!("user bob\n{line}\nport 22\n");
            assert!(
                crate::terminal::frozen_config_text(&output).is_err(),
                "{line}"
            );
        }
    }

    #[test]
    fn parse_exact_forward_proxy_jump() {
        let jump = crate::terminal::parse_effective_forwards("user bob\nproxyjump a@b:2222,c\n");
        assert_eq!(jump.proxy_jump.as_deref(), Some("a@b:2222,c"));
        let upper = crate::terminal::parse_effective_forwards("ProxyJump jumphost\n");
        assert_eq!(upper.proxy_jump.as_deref(), Some("jumphost"));
        for output in [
            "user bob\n",
            "proxyjump none\n",
            "proxyjump NONE\n",
            "proxyjump\n",
        ] {
            assert_eq!(
                crate::terminal::parse_effective_forwards(output).proxy_jump,
                None,
                "{output}"
            );
        }
        let jump = effective(&["[127.0.0.1]:1 [h]:2"], &[], &[]);
        let jump = EffectiveForwards {
            proxy_jump: Some("jumphost".into()),
            ..jump
        };
        let error = ensure_unattended(&jump, ForwardKind::Local, "127.0.0.1:1:h:2").unwrap_err();
        assert_eq!(
            error.to_string(),
            "Auto-start skipped: this tunnel uses ProxyJump. Start it manually."
        );
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

        // Through the unattended seam the snapshot of the inherited config
        // is refused before anything is spawned.
        let frozen_dir = tempfile::tempdir().unwrap();
        let (result, seen, manager) = exact_forward_attempt(
            &session,
            spec,
            Some(&inherited),
            frozen_dir.path(),
            &crate::terminal::query_effective_config,
        );
        let error = result.unwrap_err();
        assert!(
            error.downcast_ref::<InheritedForwards>().is_some(),
            "{error:#}"
        );
        assert!(seen.is_empty());
        assert!(manager.processes.is_empty());
        assert!(dir_entries(frozen_dir.path()).is_empty());
    }

    /// Real OpenSSH, no network: the user configuration changes after the
    /// first `ssh -G`, yet the frozen launch still sets up only the selection.
    #[test]
    fn real_openssh_exact_forward_frozen_launch_ignores_later_config_change() {
        let dir = tempfile::tempdir().unwrap();
        let frozen_dir = tempfile::tempdir().unwrap();
        let spec = "127.0.0.1:18098:127.0.0.1:7";
        let session = single_forward_session("exactfwd.invalid", &[spec]);
        let original = dir.path().join("ssh_config");
        std::fs::write(
            &original,
            "Host exactfwd.invalid\n  ServerAliveInterval 30\n",
        )
        .unwrap();
        let queries = std::cell::Cell::new(0);
        let query = |args: &[String]| -> Result<String> {
            let output = crate::terminal::query_effective_config(args)?;
            if queries.replace(queries.get() + 1) == 0 {
                let mut file = std::fs::OpenOptions::new().append(true).open(&original)?;
                std::io::Write::write_all(
                    &mut file,
                    b"  LocalForward 127.0.0.1:18099 127.0.0.1:7\n  RemoteForward 18100 127.0.0.1:7\n",
                )?;
            }
            Ok(output)
        };
        let spawned = std::cell::Cell::new(false);
        let spawn = |selected: &Session, launch: TunnelConfig<'_>| -> Result<TunnelProcess> {
            spawned.set(true);
            let args = launch.launch_args(selected)?;
            assert_ne!(config_arg(&args), original.to_str());
            // The snapshot still holds the bytes read before the change.
            assert_eq!(
                std::fs::read_to_string(launch.path().unwrap())?,
                "Host exactfwd.invalid\n  ServerAliveInterval 30\n"
            );
            let actual = parse_effective_forwards(&crate::terminal::query_effective_config(&args)?);
            ensure_exact_forward(&actual, ForwardKind::Local, spec)?;
            // The fixture really changed: the user argv now adds forwards.
            let user = crate::terminal::effective_forwards(selected, Some(&original))?;
            assert_eq!(user.local.len(), 2, "{user:?}");
            assert_eq!(user.remote.len(), 1, "{user:?}");
            anyhow::bail!("checked; nothing spawned")
        };
        let seam = Seam {
            exact: Some(ExactCheck {
                query: &query,
                frozen_dir: frozen_dir.path(),
            }),
            spawn: &spawn,
        };
        let mut manager = Manager::default();
        let status = manager
            .launch_forward(&session, ForwardKind::Local, spec, Some(&original), seam)
            .unwrap();
        assert!(spawned.get());
        assert_eq!(status, Status::Failed("checked; nothing spawned".into()));
        // One preflight on the snapshot; the source changed after it was read.
        assert_eq!(queries.get(), 1);
        assert!(dir_entries(frozen_dir.path()).is_empty());
    }

    /// Real OpenSSH, no network: ProxyJump from a `Host` stanza (not the
    /// saved profile) skips the unattended launch; a manual start does not
    /// query OpenSSH and reaches the spawn.
    #[test]
    fn real_openssh_exact_forward_skips_config_proxy_jump() {
        let dir = tempfile::tempdir().unwrap();
        let frozen_dir = tempfile::tempdir().unwrap();
        let spec = "127.0.0.1:18101:127.0.0.1:7";
        let session = single_forward_session("exactfwd.invalid", &[spec]);
        assert!(session.ssh.proxy_jump.is_empty());
        let config = dir.path().join("ssh_config");
        std::fs::write(&config, "Host exactfwd.invalid\n  ProxyJump jumphost\n").unwrap();
        let spawned = std::cell::Cell::new(0);
        let spawn = |_: &Session, launch: TunnelConfig<'_>| -> Result<TunnelProcess> {
            spawned.set(spawned.get() + 1);
            assert!(
                matches!(launch, TunnelConfig::User(_)),
                "manual reads the user config"
            );
            anyhow::bail!("fake spawn")
        };
        let mut manager = Manager::default();
        let seam = Seam {
            exact: Some(ExactCheck {
                query: &crate::terminal::query_effective_config,
                frozen_dir: frozen_dir.path(),
            }),
            spawn: &spawn,
        };
        let error = manager
            .launch_forward(&session, ForwardKind::Local, spec, Some(&config), seam)
            .unwrap_err();
        assert_eq!(
            error.to_string(),
            "Auto-start skipped: this tunnel uses ProxyJump. Start it manually."
        );
        assert!(error.downcast_ref::<ProxyJumpSkipped>().is_some());
        assert_eq!(spawned.get(), 0);
        assert!(manager.processes.is_empty());
        assert!(manager.failures.is_empty());
        assert!(dir_entries(frozen_dir.path()).is_empty());

        // Manual start (the seam `start_forward` uses): no query, spawn reached.
        let seam = Seam {
            exact: None,
            spawn: &spawn,
        };
        let status = manager
            .launch_forward(&session, ForwardKind::Local, spec, Some(&config), seam)
            .unwrap();
        assert_eq!(spawned.get(), 1);
        assert_eq!(status, Status::Failed("fake spawn".into()));
    }
}
