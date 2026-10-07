//! Validated non-secret profiles and argument-vector OpenSSH launch policy.
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{fs, io::Read, path::Path};

const MAX_PROFILE_BYTES: usize = 1_048_576;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProxyKind {
    #[default]
    None,
    HttpConnect,
    Socks5,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ControlMasterMode {
    #[default]
    Inherit,
    Disabled,
    Auto,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SshOptions {
    /// Path to an identity file. The path is stored, never the private key contents.
    pub identity_file: String,
    /// OpenSSH ProxyJump route, for example `bastion` or `user@bastion:2222,target-hop`.
    pub proxy_jump: String,
    /// Structured proxy transport. No proxy credentials are stored.
    pub proxy_kind: ProxyKind,
    pub proxy_host: String,
    pub proxy_port: Option<u16>,
    /// Structured OpenSSH connection multiplexing. Inherit leaves ~/.ssh/config untouched.
    pub control_master: ControlMasterMode,
    pub control_path: String,
    pub control_persist_seconds: Option<u32>,
    /// Authentication policies. None inherits the effective OpenSSH configuration.
    pub public_key_auth: Option<bool>,
    pub password_auth: Option<bool>,
    pub keyboard_interactive_auth: Option<bool>,
    pub gssapi_auth: Option<bool>,
    pub gssapi_delegate_credentials: Option<bool>,
    pub identities_only: Option<bool>,
    /// None inherits OpenSSH config; Some(true/false) explicitly enables/disables.
    pub agent_forwarding: Option<bool>,
    /// None inherits OpenSSH config; Some(true/false) explicitly enables/disables.
    pub x11_forwarding: Option<bool>,
    /// None inherits OpenSSH config; Some(true/false) explicitly enables/disables.
    pub compression: Option<bool>,
    pub connect_timeout_seconds: Option<u16>,
    pub server_alive_interval_seconds: Option<u16>,
    /// Optional command sent to the remote account after authentication.
    /// It is never executed by a local shell.
    pub remote_command: String,
    /// OpenSSH -L specifications. Each entry is passed as one argv token.
    pub local_forwards: Vec<String>,
    /// OpenSSH -R specifications. Each entry is passed as one argv token.
    pub remote_forwards: Vec<String>,
    /// OpenSSH -D specifications. Each entry is passed as one argv token.
    pub dynamic_forwards: Vec<String>,
}

impl SshOptions {
    fn validate(&self) -> Result<()> {
        if !self.identity_file.is_empty() {
            ensure!(
                valid_single_argument(&self.identity_file, 4096),
                "identity file path must be at most 4096 bytes and contain no control characters"
            );
        }
        if !self.proxy_jump.is_empty() {
            ensure!(
                valid_proxy_jump(&self.proxy_jump),
                "ProxyJump must be a comma-separated SSH destination chain without spaces, control characters or option prefixes"
            );
        }
        if self.proxy_kind != ProxyKind::None {
            ensure!(
                self.proxy_jump.is_empty(),
                "ProxyJump and structured HTTP/SOCKS proxy transport are mutually exclusive"
            );
            ensure!(
                valid_proxy_endpoint(&self.proxy_host),
                "proxy host must be a hostname or IPv4/IPv6 address without spaces, zone identifiers, options or shell syntax"
            );
            ensure!(
                self.proxy_port.is_some_and(|port| port > 0),
                "proxy port must be 1–65535 when structured proxy transport is enabled"
            );
        }
        if self.control_master == ControlMasterMode::Auto {
            ensure!(
                valid_single_argument(&self.control_path, 4096),
                "ControlPath is required for app-managed multiplexing, must be at most 4096 bytes and contain no control characters"
            );
            #[cfg(unix)]
            ensure!(
                self.control_path.len() <= 80,
                "ControlPath is too long for reliable Unix-domain socket creation; use a shorter path (80 bytes or fewer before OpenSSH expansion)"
            );
        }
        if let Some(seconds) = self.control_persist_seconds {
            ensure!(seconds > 0, "ControlPersist must be greater than zero");
        }
        for (name, value) in [
            ("connect timeout", self.connect_timeout_seconds),
            ("server alive interval", self.server_alive_interval_seconds),
        ] {
            if let Some(value) = value {
                ensure!(value > 0, "{name} must be greater than zero");
            }
        }
        if !self.remote_command.is_empty() {
            ensure!(
                valid_single_argument(&self.remote_command, 8192),
                "remote command must be at most 8192 bytes and contain no control characters"
            );
        }
        validate_forward_specs("local forward", &self.local_forwards)?;
        validate_forward_specs("remote forward", &self.remote_forwards)?;
        validate_forward_specs("dynamic forward", &self.dynamic_forwards)?;
        Ok(())
    }

    fn append_args(&self, args: &mut Vec<String>) -> Result<()> {
        self.validate()?;
        if !self.identity_file.is_empty() {
            args.extend(["-i".into(), self.identity_file.clone()]);
        }
        if !self.proxy_jump.is_empty() {
            args.extend(["-J".into(), self.proxy_jump.clone()]);
        }
        append_boolean_option(args, "PubkeyAuthentication", self.public_key_auth);
        append_boolean_option(args, "PasswordAuthentication", self.password_auth);
        append_boolean_option(
            args,
            "KbdInteractiveAuthentication",
            self.keyboard_interactive_auth,
        );
        append_boolean_option(args, "GSSAPIAuthentication", self.gssapi_auth);
        append_boolean_option(
            args,
            "GSSAPIDelegateCredentials",
            self.gssapi_delegate_credentials,
        );
        append_boolean_option(args, "IdentitiesOnly", self.identities_only);
        match self.control_master {
            ControlMasterMode::Inherit => {}
            ControlMasterMode::Disabled => args.extend(["-o".into(), "ControlMaster=no".into()]),
            ControlMasterMode::Auto => {
                args.extend(["-o".into(), "ControlMaster=auto".into()]);
                args.extend(["-o".into(), format!("ControlPath={}", self.control_path)]);
                if let Some(seconds) = self.control_persist_seconds {
                    args.extend(["-o".into(), format!("ControlPersist={seconds}")]);
                }
            }
        }
        match self.agent_forwarding {
            Some(true) => args.push("-A".into()),
            Some(false) => args.push("-a".into()),
            None => {}
        }
        match self.x11_forwarding {
            Some(true) => args.push("-X".into()),
            Some(false) => args.push("-x".into()),
            None => {}
        }
        match self.compression {
            Some(true) => args.push("-C".into()),
            Some(false) => args.extend(["-o".into(), "Compression=no".into()]),
            None => {}
        }
        if let Some(seconds) = self.connect_timeout_seconds {
            args.extend(["-o".into(), format!("ConnectTimeout={seconds}")]);
        }
        if let Some(seconds) = self.server_alive_interval_seconds {
            args.extend(["-o".into(), format!("ServerAliveInterval={seconds}")]);
        }
        let has_forwarding = !self.local_forwards.is_empty()
            || !self.remote_forwards.is_empty()
            || !self.dynamic_forwards.is_empty();
        if has_forwarding {
            args.extend(["-o".into(), "ExitOnForwardFailure=yes".into()]);
        }
        for spec in &self.local_forwards {
            args.extend(["-L".into(), spec.clone()]);
        }
        for spec in &self.remote_forwards {
            args.extend(["-R".into(), spec.clone()]);
        }
        for spec in &self.dynamic_forwards {
            args.extend(["-D".into(), spec.clone()]);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Session {
    pub name: String,
    pub host: String,
    pub user: String,
    pub port: Option<u16>,
    /// false = ask (interactive trust); true = yes (pre-trusted keys only).
    pub strict: bool,
    /// Optional organizational folder. This is non-secret metadata only.
    #[serde(default)]
    pub folder: String,
    /// User-defined non-secret labels used for library filtering.
    #[serde(default)]
    pub tags: Vec<String>,
    /// Pinned profiles are surfaced first by the UI.
    #[serde(default)]
    pub favorite: bool,
    #[serde(default)]
    pub ssh: SshOptions,
}
impl Default for Session {
    fn default() -> Self {
        Self {
            name: "New session".into(),
            host: String::new(),
            user: String::new(),
            port: None,
            strict: false,
            folder: String::new(),
            tags: Vec::new(),
            favorite: false,
            ssh: SshOptions::default(),
        }
    }
}
impl Session {
    /// Build argv, never a shell string. Configuration and agent are OpenSSH's.
    pub fn ssh_args(&self) -> Result<Vec<String>> {
        validate_profile_metadata(self)?;
        ensure!(
            !self.name.trim().is_empty()
                && self.name.len() <= 256
                && !self.name.chars().any(char::is_control),
            "session name must be 1–256 bytes without control characters"
        );
        ensure!(
            valid_token(&self.host, true),
            "host must be a hostname, SSH config alias or IPv4/IPv6 address (no username, spaces, options or shell syntax)"
        );
        ensure!(
            self.user.is_empty() || valid_token(&self.user, false),
            "username contains unsupported characters"
        );
        ensure!(self.port != Some(0), "port must be 1–65535");
        let mut args = vec![
            "-tt".into(),
            "-o".into(),
            format!(
                "StrictHostKeyChecking={}",
                if self.strict { "yes" } else { "ask" }
            ),
        ];
        self.ssh.append_args(&mut args)?;
        if !self.user.is_empty() {
            args.extend(["-l".into(), self.user.clone()]);
        }
        if let Some(port) = self.port {
            args.extend(["-p".into(), port.to_string()]);
        }
        args.extend(["--".into(), self.host.clone()]);
        if !self.ssh.remote_command.is_empty() {
            args.push(self.ssh.remote_command.clone());
        }
        Ok(args)
    }
}

fn append_boolean_option(args: &mut Vec<String>, name: &str, value: Option<bool>) {
    if let Some(enabled) = value {
        args.extend([
            "-o".into(),
            format!("{name}={}", if enabled { "yes" } else { "no" }),
        ]);
    }
}

fn valid_single_argument(value: &str, max_len: usize) -> bool {
    !value.is_empty() && value.len() <= max_len && !value.chars().any(char::is_control)
}

fn valid_proxy_endpoint(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 253
        && !value.starts_with('-')
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._-:".contains(&byte))
}

fn valid_proxy_jump(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 1024
        && !value.starts_with('-')
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-@:,%[]".contains(&b))
}

pub fn forward_requires_risk_ack(spec: &str, dynamic: bool) -> bool {
    if spec.starts_with("127.0.0.1:")
        || spec.starts_with("localhost:")
        || spec.starts_with("[::1]:")
    {
        return false;
    }
    if dynamic {
        return spec.contains(':');
    }
    // With no explicit bind address, -L/-R have listen-port:host:host-port (two colons).
    // An explicit bind adds another separator; bracketed IPv6 naturally also lands here.
    spec.bytes().filter(|byte| *byte == b':').count() > 2
}

pub fn session_requires_forward_risk_ack(session: &Session) -> bool {
    session
        .ssh
        .local_forwards
        .iter()
        .chain(session.ssh.remote_forwards.iter())
        .any(|spec| forward_requires_risk_ack(spec, false))
        || session
            .ssh
            .dynamic_forwards
            .iter()
            .any(|spec| forward_requires_risk_ack(spec, true))
}

fn validate_forward_specs(label: &str, specs: &[String]) -> Result<()> {
    ensure!(specs.len() <= 32, "at most 32 {label}s are supported");
    for spec in specs {
        ensure!(
            valid_single_argument(spec, 2048),
            "{label} must be 1–2048 bytes and contain no control characters"
        );
    }
    Ok(())
}

fn valid_token(value: &str, host: bool) -> bool {
    !value.is_empty()
        && value.len() <= 253
        && !value.starts_with('-')
        && value.bytes().all(|b| {
            b.is_ascii_alphanumeric() || b"._-".contains(&b) || (host && b":%".contains(&b))
        })
}
fn validate_profile_metadata(session: &Session) -> Result<()> {
    ensure!(
        session.folder.len() <= 512 && !session.folder.chars().any(char::is_control),
        "profile folder must be at most 512 bytes without control characters"
    );
    ensure!(
        session.tags.len() <= 32,
        "at most 32 profile tags are supported"
    );
    for tag in &session.tags {
        ensure!(
            !tag.trim().is_empty() && tag.len() <= 64 && !tag.chars().any(char::is_control),
            "profile tags must be 1–64 bytes without control characters"
        );
    }
    Ok(())
}

/// Stable selector used by the UI and import logic. Names may repeat in different folders.
pub fn session_profile_key(session: &Session) -> String {
    format!(
        "{}:{}{}",
        session.folder.len(),
        session.folder,
        session.name
    )
}

fn selector_matches(session: &Session, selector: &str) -> bool {
    selector == session_profile_key(session) || selector == session.name
}

fn same_profile_slot(left: &Session, right: &Session) -> bool {
    left.folder == right.folder && left.name == right.name
}

/// Return true when a saved SSH profile matches a case-insensitive library query.
pub fn session_matches_query(session: &Session, query: &str) -> bool {
    let query = query.trim().to_lowercase();
    query.is_empty()
        || session.name.to_lowercase().contains(&query)
        || session.host.to_lowercase().contains(&query)
        || session.user.to_lowercase().contains(&query)
        || session.folder.to_lowercase().contains(&query)
        || session
            .tags
            .iter()
            .any(|tag| tag.to_lowercase().contains(&query))
}

/// Replace the selected profile, or save a new profile when no selection is active.
///
/// The selector is the folder/name profile key. Plain names remain accepted for compatibility
/// with older callers, but the UI always uses the deterministic composite key.
pub fn save_session_edit(
    sessions: &[Session],
    selected_profile: Option<&str>,
    session: Session,
) -> Result<Vec<Session>> {
    session.ssh_args()?;
    let mut next = sessions.to_vec();

    if let Some(selected_profile) = selected_profile {
        let index = next
            .iter()
            .position(|profile| selector_matches(profile, selected_profile))
            .context("selected profile no longer exists")?;
        ensure!(
            !next
                .iter()
                .enumerate()
                .any(|(other, profile)| other != index && same_profile_slot(profile, &session)),
            "a profile named {:?} already exists in folder {:?}",
            session.name,
            session.folder
        );
        next[index] = session;
        return Ok(next);
    }

    ensure!(
        !next
            .iter()
            .any(|profile| same_profile_slot(profile, &session)),
        "a profile named {:?} already exists in folder {:?}",
        session.name,
        session.folder
    );
    next.push(session);
    Ok(next)
}

/// Remove one saved profile by deterministic folder/name selector.
pub fn delete_session(sessions: &[Session], selector: &str) -> Result<Vec<Session>> {
    let index = sessions
        .iter()
        .position(|profile| selector_matches(profile, selector))
        .context("selected profile no longer exists")?;
    let mut next = sessions.to_vec();
    next.remove(index);
    Ok(next)
}

fn copy_name_candidate(source: &str, number: Option<u32>) -> String {
    let suffix = match number {
        Some(number) => format!(" copy {number}"),
        None => " copy".to_owned(),
    };
    let max_source_bytes = 256 - suffix.len();
    let mut end = source.len().min(max_source_bytes);
    while !source.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}{}", &source[..end], suffix)
}

/// Build a unique editable duplicate without persisting it.
pub fn duplicate_session_draft(sessions: &[Session], source: &Session) -> Session {
    let mut copy = source.clone();
    let first = copy_name_candidate(&source.name, None);
    if !sessions
        .iter()
        .any(|profile| profile.folder == source.folder && profile.name == first)
    {
        copy.name = first;
        return copy;
    }
    let mut suffix = 2_u32;
    loop {
        let candidate = copy_name_candidate(&source.name, Some(suffix));
        if !sessions
            .iter()
            .any(|profile| profile.folder == source.folder && profile.name == candidate)
        {
            copy.name = candidate;
            return copy;
        }
        suffix += 1;
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionImportMode {
    Merge,
    Replace,
}

fn ensure_unique_profile_slots(sessions: &[Session], label: &str) -> Result<()> {
    for (index, session) in sessions.iter().enumerate() {
        ensure!(
            !sessions[..index]
                .iter()
                .any(|profile| same_profile_slot(profile, session)),
            "{label} contains duplicate profile name {:?} in folder {:?}",
            session.name,
            session.folder
        );
    }
    Ok(())
}

/// Validate an existing import source and the complete result without writing either file.
pub fn import_sessions(
    path: &Path,
    existing: &[Session],
    mode: SessionImportMode,
) -> Result<Vec<Session>> {
    // A missing active store is normal at startup; a missing import is never an empty library.
    // Open once and parse that handle, rather than checking existence and reopening the path.
    let file = fs::File::open(path).context("open profile import (source must exist)")?;
    let imported = read_sessions(file).context("load profile import")?;
    ensure_unique_profile_slots(&imported, "profile import")?;

    let candidate = match mode {
        SessionImportMode::Replace => imported,
        SessionImportMode::Merge => {
            ensure!(
                existing.len() + imported.len() <= 1000,
                "merged profile library would exceed 1000 profiles"
            );
            for session in &imported {
                ensure!(
                    !existing
                        .iter()
                        .any(|profile| same_profile_slot(profile, session)),
                    "profile import conflicts with existing profile {:?} in folder {:?}",
                    session.name,
                    session.folder
                );
            }
            let mut merged = existing.to_vec();
            merged.extend(imported);
            merged
        }
    };
    ensure_unique_profile_slots(&candidate, "resulting profile library")?;
    validated_session_bytes(&candidate).context("validate imported profile library")?;
    Ok(candidate)
}

fn validated_session_bytes(sessions: &[Session]) -> Result<Vec<u8>> {
    ensure!(
        sessions.len() <= 1000,
        "at most 1000 profiles are supported"
    );
    for session in sessions {
        session.ssh_args()?;
    }
    let mut bytes = serde_json::to_vec_pretty(sessions)?;
    bytes.push(b'\n');
    ensure!(
        bytes.len() <= MAX_PROFILE_BYTES,
        "profile file exceeds 1 MiB"
    );
    Ok(bytes)
}

fn profile_parent(path: &Path) -> &Path {
    path.parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(Path::new("."))
}

/// Export the validated non-secret profile model without overwriting an existing destination.
pub fn export_sessions(path: &Path, sessions: &[Session]) -> Result<()> {
    let bytes = validated_session_bytes(sessions)?;
    let parent = profile_parent(path);
    fs::create_dir_all(parent).context("create export directory")?;
    let mut file =
        tempfile::NamedTempFile::new_in(parent).context("create temporary export file")?;
    std::io::Write::write_all(&mut file, &bytes)?;
    file.as_file().sync_all()?;
    file.persist_noclobber(path)
        .context("export destination already exists or cannot be created")?;
    Ok(())
}

/// Atomically replace validated non-secret profiles; never truncate an existing file on failure.
pub fn save_sessions(path: &Path, sessions: &[Session]) -> Result<()> {
    let bytes = validated_session_bytes(sessions)?;
    let parent = profile_parent(path);
    fs::create_dir_all(parent).context("create profile directory")?;
    let mut file =
        tempfile::NamedTempFile::new_in(parent).context("create temporary profile file")?;
    std::io::Write::write_all(&mut file, &bytes)?;
    file.as_file().sync_all()?;
    file.persist(path).context("replace profile file")?;
    Ok(())
}
/// Missing active store is an empty collection; import sources must exist.
pub fn load_sessions(path: &Path) -> Result<Vec<Session>> {
    let file = match fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error).context("open profile file"),
    };
    read_sessions(file)
}

fn read_sessions(file: fs::File) -> Result<Vec<Session>> {
    let mut bytes = Vec::new();
    file.take((MAX_PROFILE_BYTES + 1) as u64)
        .read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() <= MAX_PROFILE_BYTES,
        "profile file exceeds 1 MiB"
    );
    let sessions: Vec<Session> =
        serde_json::from_slice(&bytes).context("parse profiles (file left unchanged)")?;
    ensure!(
        sessions.len() <= 1000,
        "at most 1000 profiles are supported"
    );
    for session in &sessions {
        session.ssh_args()?;
    }
    Ok(sessions)
}

pub mod appearance;
pub mod command_palette;
pub mod history;
pub mod keyboard;
pub mod proxy;
pub mod remote_edit;
pub mod scp;
pub mod scp_panel;
pub mod sftp;
pub mod sftp_browser;
pub mod startup;
pub mod support;
pub mod tab_management;
pub mod terminal;
pub mod terminal_ux;
pub mod tmux;
pub mod workspace;

#[cfg(feature = "iced-ui")]
pub mod iced_app;

pub mod app;
