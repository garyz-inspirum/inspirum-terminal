//! Validated non-secret profiles and argument-vector OpenSSH launch policy.
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{fs, io::Read, path::Path};

const MAX_PROFILE_BYTES: usize = 1_048_576;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Session {
    pub name: String,
    pub host: String,
    pub user: String,
    pub port: Option<u16>,
    /// false = ask (interactive trust); true = yes (pre-trusted keys only).
    pub strict: bool,
}
impl Default for Session {
    fn default() -> Self {
        Self {
            name: "New session".into(),
            host: String::new(),
            user: String::new(),
            port: None,
            strict: false,
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
        if !self.user.is_empty() {
            args.extend(["-l".into(), self.user.clone()]);
        }
        if let Some(port) = self.port {
            args.extend(["-p".into(), port.to_string()]);
        }
        args.extend(["--".into(), self.host.clone()]);
        Ok(args)
    }
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
