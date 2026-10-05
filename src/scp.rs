//! Managed SCP upload/download operations using the system OpenSSH scp client.
use crate::{ControlMasterMode, ProxyKind, Session, sftp, terminal};
use anyhow::{Context, Result, ensure};
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Child, Command, ExitStatus, Stdio},
    sync::atomic::{AtomicU64, Ordering},
};
use tempfile::TempPath;

static UPLOAD_ID: AtomicU64 = AtomicU64::new(1);

const SCP_HELP: &str = "OpenSSH scp client unavailable. Install OpenSSH Client and ensure scp is on PATH, then restart Inspirum.";

fn check_scp() -> Result<()> {
    let output = Command::new("scp").arg("-h").output().context(SCP_HELP)?;
    let usage = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    ensure!(usage.to_ascii_lowercase().contains("scp"), "{SCP_HELP}");
    Ok(())
}

fn remote_path(value: &str) -> Result<&str> {
    ensure!(!value.is_empty(), "remote SCP path is required");
    ensure!(
        value.len() <= 8192 && !value.chars().any(char::is_control),
        "remote SCP path must be at most 8192 bytes and contain no control characters"
    );
    ensure!(
        value == "/" || !value.ends_with('/'),
        "remote SCP destination must name a file, not end with a directory separator"
    );
    ensure!(value != "/", "remote SCP path must name a file");
    Ok(value)
}

fn remote_parent_name(path: &str) -> Result<(&str, &str)> {
    remote_path(path)?;
    match path.rsplit_once('/') {
        Some(("", name)) => Ok(("/", name)),
        Some((parent, name)) => Ok((parent, name)),
        None => Ok((".", path)),
    }
}

fn common_args_with_proxy_helper(
    session: &Session,
    config: Option<&Path>,
    helper: &Path,
) -> Result<Vec<String>> {
    session.ssh_args()?;
    let mut args = vec![
        "-o".into(),
        format!(
            "StrictHostKeyChecking={}",
            if session.strict { "yes" } else { "ask" }
        ),
    ];
    if let Some(config) = config {
        let value = config
            .to_str()
            .context("SSH config path must be valid Unicode")?;
        ensure!(!value.contains('\0'), "SSH config path contains NUL");
        args.extend(["-F".into(), value.to_owned()]);
    }
    if !session.ssh.identity_file.is_empty() {
        args.extend(["-i".into(), session.ssh.identity_file.clone()]);
    }
    if !session.ssh.proxy_jump.is_empty() {
        args.extend(["-J".into(), session.ssh.proxy_jump.clone()]);
    }
    match session.ssh.control_master {
        ControlMasterMode::Inherit => {}
        ControlMasterMode::Disabled => args.extend(["-o".into(), "ControlMaster=no".into()]),
        ControlMasterMode::Auto => {
            args.extend(["-o".into(), "ControlMaster=auto".into()]);
            args.extend([
                "-o".into(),
                format!("ControlPath={}", session.ssh.control_path),
            ]);
            if let Some(seconds) = session.ssh.control_persist_seconds {
                args.extend(["-o".into(), format!("ControlPersist={seconds}")]);
            }
        }
    }
    if session.ssh.proxy_kind != ProxyKind::None {
        let target = terminal::resolve_host_key_target(session, config)?;
        if let Some(command) =
            terminal::proxy_command_for_target(session, helper, &target.hostname, target.port)?
        {
            args.extend(["-o".into(), format!("ProxyCommand={command}")]);
        }
    }
    for (name, value) in [
        ("PubkeyAuthentication", session.ssh.public_key_auth),
        ("PasswordAuthentication", session.ssh.password_auth),
        (
            "KbdInteractiveAuthentication",
            session.ssh.keyboard_interactive_auth,
        ),
        ("GSSAPIAuthentication", session.ssh.gssapi_auth),
        (
            "GSSAPIDelegateCredentials",
            session.ssh.gssapi_delegate_credentials,
        ),
        ("IdentitiesOnly", session.ssh.identities_only),
    ] {
        crate::append_boolean_option(&mut args, name, value);
    }
    match session.ssh.compression {
        Some(true) => args.push("-C".into()),
        Some(false) => args.extend(["-o".into(), "Compression=no".into()]),
        None => {}
    }
    if let Some(seconds) = session.ssh.connect_timeout_seconds {
        args.extend(["-o".into(), format!("ConnectTimeout={seconds}")]);
    }
    if let Some(seconds) = session.ssh.server_alive_interval_seconds {
        args.extend(["-o".into(), format!("ServerAliveInterval={seconds}")]);
    }
    if !session.user.is_empty() {
        args.extend(["-o".into(), format!("User={}", session.user)]);
    }
    if let Some(port) = session.port {
        args.extend(["-P".into(), port.to_string()]);
    }
    Ok(args)
}

fn remote_spec(session: &Session, path: &str) -> Result<String> {
    let path = remote_path(path)?;
    let host = if session.host.contains(':') {
        format!("[{}]", session.host)
    } else {
        session.host.clone()
    };
    Ok(format!("{host}:{path}"))
}

pub fn upload_args_with_proxy_helper(
    session: &Session,
    config: Option<&Path>,
    helper: &Path,
    local: &Path,
    remote: &str,
) -> Result<Vec<String>> {
    ensure!(local.is_file(), "SCP upload source must be a regular file");
    let local = local
        .to_str()
        .context("local SCP path must be valid Unicode")?;
    ensure!(
        !local.chars().any(char::is_control),
        "local SCP path is invalid"
    );
    let mut args = common_args_with_proxy_helper(session, config, helper)?;
    args.extend(["--".into(), local.to_owned(), remote_spec(session, remote)?]);
    Ok(args)
}

pub fn download_args_with_proxy_helper(
    session: &Session,
    config: Option<&Path>,
    helper: &Path,
    remote: &str,
    local: &Path,
) -> Result<Vec<String>> {
    let local = local
        .to_str()
        .context("local SCP path must be valid Unicode")?;
    ensure!(
        !local.chars().any(char::is_control),
        "local SCP path is invalid"
    );
    let mut args = common_args_with_proxy_helper(session, config, helper)?;
    args.extend(["--".into(), remote_spec(session, remote)?, local.to_owned()]);
    Ok(args)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    Upload,
    Download,
}

pub struct Transfer {
    child: Child,
    direction: Direction,
    staging: Option<TempPath>,
    destination: Option<PathBuf>,
    overwrite: bool,
    total: u64,
    session: Session,
    config: Option<PathBuf>,
    remote_staging: Option<String>,
    remote_destination: Option<String>,
}

impl Transfer {
    pub fn direction(&self) -> Direction {
        self.direction
    }

    pub fn total_bytes(&self) -> u64 {
        self.total
    }

    pub fn transferred_bytes(&self) -> u64 {
        match self.direction {
            Direction::Upload => 0,
            Direction::Download => self
                .staging
                .as_ref()
                .and_then(|path| fs::metadata(path).ok())
                .map(|metadata| metadata.len())
                .unwrap_or(0),
        }
    }

    pub fn cancel(&mut self) -> Result<()> {
        if self.child.try_wait()?.is_none() {
            self.child.kill().context("cancel SCP transfer")?;
        }
        let _ = self.child.wait();
        self.staging.take();
        if let Some(staging) = self.remote_staging.take() {
            let _ = sftp::delete_remote(&self.session, self.config.as_deref(), &staging, false);
        }
        Ok(())
    }

    pub fn poll(&mut self) -> Result<Option<()>> {
        let Some(status) = self.child.try_wait()? else {
            return Ok(None);
        };
        let mut stderr = Vec::new();
        if let Some(mut pipe) = self.child.stderr.take() {
            std::io::Read::read_to_end(&mut pipe, &mut stderr)?;
        }
        ensure_success(status, &stderr)?;
        match self.direction {
            Direction::Download => {
                let staging = self
                    .staging
                    .take()
                    .context("SCP download staging file is missing")?;
                let actual = fs::metadata(&staging)?.len();
                ensure!(
                    actual == self.total,
                    "SCP download size verification failed: expected {} bytes, received {actual}",
                    self.total
                );
                let destination = self
                    .destination
                    .as_ref()
                    .context("SCP download destination is missing")?;
                if self.overwrite {
                    staging
                        .persist(destination)
                        .map_err(|error| error.error)
                        .context("commit completed SCP download")?;
                } else {
                    staging
                        .persist_noclobber(destination)
                        .map_err(|error| error.error)
                        .context("commit completed SCP download without overwriting destination")?;
                }
            }
            Direction::Upload => {
                let staging = self
                    .remote_staging
                    .take()
                    .context("SCP remote staging path is missing")?;
                let destination = self
                    .remote_destination
                    .as_ref()
                    .context("SCP remote destination is missing")?;
                let actual = sftp::remote_size(&self.session, self.config.as_deref(), &staging)?;
                if actual != self.total {
                    let _ =
                        sftp::delete_remote(&self.session, self.config.as_deref(), &staging, false);
                    anyhow::bail!(
                        "SCP upload size verification failed: expected {} bytes, found {actual}",
                        self.total
                    );
                }
                if !self.overwrite {
                    let (parent, name) = remote_parent_name(destination)?;
                    if sftp::list_remote(&self.session, self.config.as_deref(), parent)?
                        .iter()
                        .any(|entry| entry.name == name)
                    {
                        let _ = sftp::delete_remote(
                            &self.session,
                            self.config.as_deref(),
                            &staging,
                            false,
                        );
                        anyhow::bail!(
                            "remote destination appeared during transfer; explicit overwrite is required"
                        );
                    }
                }
                if let Err(error) = sftp::rename_remote(
                    &self.session,
                    self.config.as_deref(),
                    &staging,
                    destination,
                ) {
                    let _ =
                        sftp::delete_remote(&self.session, self.config.as_deref(), &staging, false);
                    return Err(error).context("commit verified SCP upload");
                }
            }
        }
        Ok(Some(()))
    }
}

impl Drop for Transfer {
    fn drop(&mut self) {
        if self.child.try_wait().ok().flatten().is_none() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
        self.staging.take();
        if let Some(staging) = self.remote_staging.take() {
            let _ = sftp::delete_remote(&self.session, self.config.as_deref(), &staging, false);
        }
    }
}

fn ensure_success(status: ExitStatus, stderr: &[u8]) -> Result<()> {
    ensure!(
        status.success(),
        "SCP transfer failed: {}",
        String::from_utf8_lossy(stderr).trim()
    );
    Ok(())
}

fn spawn(args: Vec<String>) -> Result<Child> {
    check_scp()?;
    Command::new("scp")
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .context("start OpenSSH scp transfer")
}

pub fn start_upload(
    session: &Session,
    config: Option<&Path>,
    local: &Path,
    remote: &str,
    overwrite: bool,
) -> Result<Transfer> {
    remote_path(remote)?;
    if !overwrite {
        let (parent, name) = remote_parent_name(remote)?;
        if sftp::list_remote(session, config, parent)?
            .iter()
            .any(|entry| entry.name == name)
        {
            anyhow::bail!("remote destination already exists; explicit overwrite is required");
        }
    }
    let total = fs::metadata(local)?.len();
    let suffix = UPLOAD_ID.fetch_add(1, Ordering::Relaxed);
    let staging = format!("{remote}.inspirum-scp-{}-{suffix}", std::process::id());
    let helper = if session.ssh.proxy_kind == ProxyKind::None {
        PathBuf::new()
    } else {
        std::env::current_exe().context("locate Inspirum proxy helper executable")?
    };
    let args = upload_args_with_proxy_helper(session, config, &helper, local, &staging)?;
    Ok(Transfer {
        child: spawn(args)?,
        direction: Direction::Upload,
        staging: None,
        destination: None,
        overwrite,
        total,
        session: session.clone(),
        config: config.map(Path::to_owned),
        remote_staging: Some(staging),
        remote_destination: Some(remote.to_owned()),
    })
}

pub fn start_download(
    session: &Session,
    config: Option<&Path>,
    remote: &str,
    local: &Path,
    overwrite: bool,
) -> Result<Transfer> {
    if local.exists() && !overwrite {
        anyhow::bail!("local destination already exists; explicit overwrite is required");
    }
    let total = sftp::remote_size(session, config, remote)?;
    let parent = local.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent).context("create local SCP download directory")?;
    let staging = tempfile::Builder::new()
        .prefix(".inspirum-scp-")
        .tempfile_in(parent)
        .context("create SCP download staging file")?
        .into_temp_path();
    let helper = if session.ssh.proxy_kind == ProxyKind::None {
        PathBuf::new()
    } else {
        std::env::current_exe().context("locate Inspirum proxy helper executable")?
    };
    let args = download_args_with_proxy_helper(session, config, &helper, remote, &staging)?;
    Ok(Transfer {
        child: spawn(args)?,
        direction: Direction::Download,
        staging: Some(staging),
        destination: Some(local.to_owned()),
        overwrite,
        total,
        session: session.clone(),
        config: config.map(Path::to_owned),
        remote_staging: None,
        remote_destination: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_spec_keeps_ipv6_bracketed_and_paths_literal() {
        let session = Session {
            host: "2001:db8::1".into(),
            ..Session::default()
        };
        assert_eq!(
            remote_spec(&session, "dir/file name;literal").unwrap(),
            "[2001:db8::1]:dir/file name;literal"
        );
        assert!(remote_spec(&session, "bad\npath").is_err());
        assert!(remote_spec(&session, "directory/").is_err());
        assert_eq!(remote_parent_name("/root.bin").unwrap(), ("/", "root.bin"));
        assert_eq!(
            remote_parent_name("dir/file.bin").unwrap(),
            ("dir", "file.bin")
        );
    }
}
