//! Validated non-secret profiles and argument-vector OpenSSH launch policy.
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{fs, io::Read, path::Path};

const MAX_PROFILE_BYTES: usize = 1_048_576;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SshOptions {
    /// Path to an identity file. The path is stored, never the private key contents.
    pub identity_file: String,
    /// OpenSSH ProxyJump route, for example `bastion` or `user@bastion:2222,target-hop`.
    pub proxy_jump: String,
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
            ssh: SshOptions::default(),
        }
    }
}
impl Session {
    /// Build argv, never a shell string. Configuration and agent are OpenSSH's.
    pub fn ssh_args(&self) -> Result<Vec<String>> {
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

fn valid_proxy_jump(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 1024
        && !value.starts_with('-')
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-@:,%[]".contains(&b))
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
/// Atomically replace validated non-secret profiles; never truncate an existing file on failure.
pub fn save_sessions(path: &Path, sessions: &[Session]) -> Result<()> {
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
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    fs::create_dir_all(parent).context("create profile directory")?;
    let mut file =
        tempfile::NamedTempFile::new_in(parent).context("create temporary profile file")?;
    std::io::Write::write_all(&mut file, &bytes)?;
    file.as_file().sync_all()?;
    file.persist(path).context("replace profile file")?;
    Ok(())
}
/// Missing file is an empty collection; malformed, oversized or unsafe profiles are errors.
pub fn load_sessions(path: &Path) -> Result<Vec<Session>> {
    let file = match fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error).context("open profile file"),
    };
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

pub mod terminal;

pub mod app;
