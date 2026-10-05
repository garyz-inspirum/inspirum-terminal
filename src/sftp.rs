//! Machine-readable SFTP operations backed by the system OpenSSH sftp client.
use crate::{Session, terminal};
use anyhow::{Context, Result, ensure};
use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemoteEntry {
    pub name: String,
    pub is_dir: bool,
    pub size: Option<u64>,
}

fn quote_remote(value: &str) -> Result<String> {
    ensure!(!value.is_empty(), "remote path is required");
    ensure!(
        !value.chars().any(char::is_control),
        "remote path contains control characters"
    );
    Ok(format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\"")))
}

fn run_batch(
    session: &Session,
    config: Option<&Path>,
    commands: &[String],
) -> Result<String> {
    let mut args = terminal::sftp_launch_args(session, config)?;
    args.insert(0, "-b".into());
    args.insert(1, "-".into());
    let mut child = Command::new("sftp")
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("start OpenSSH sftp batch operation")?;
    {
        let stdin = child.stdin.as_mut().context("open sftp batch stdin")?;
        for command in commands {
            writeln!(stdin, "{command}")?;
        }
    }
    let output = child.wait_with_output()?;
    ensure!(
        output.status.success(),
        "SFTP operation failed: {}",
        String::from_utf8_lossy(&output.stderr).trim()
    );
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

pub fn list_remote(
    session: &Session,
    config: Option<&Path>,
    remote_dir: &str,
) -> Result<Vec<RemoteEntry>> {
    let output = run_batch(
        session,
        config,
        &[format!("ls -la {}", quote_remote(remote_dir)?)],
    )?;
    let mut entries = Vec::new();
    for line in output.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with("sftp>") || line.starts_with("Connected to ") {
            continue;
        }
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.len() < 9 || !matches!(fields[0].as_bytes().first(), Some(b'-' | b'd' | b'l')) {
            continue;
        }
        let name = fields[8..].join(" ");
        if name == "." || name == ".." {
            continue;
        }
        entries.push(RemoteEntry {
            name,
            is_dir: fields[0].starts_with('d'),
            size: fields[4].parse().ok(),
        });
    }
    Ok(entries)
}

pub fn mkdir_remote(session: &Session, config: Option<&Path>, path: &str) -> Result<()> {
    run_batch(session, config, &[format!("mkdir {}", quote_remote(path)?)])?;
    Ok(())
}

pub fn rename_remote(
    session: &Session,
    config: Option<&Path>,
    from: &str,
    to: &str,
) -> Result<()> {
    run_batch(
        session,
        config,
        &[format!("rename {} {}", quote_remote(from)?, quote_remote(to)?)],
    )?;
    Ok(())
}

pub fn delete_remote(
    session: &Session,
    config: Option<&Path>,
    path: &str,
    directory: bool,
) -> Result<()> {
    let command = if directory { "rmdir" } else { "rm" };
    run_batch(
        session,
        config,
        &[format!("{command} {}", quote_remote(path)?)],
    )?;
    Ok(())
}

pub struct Transfer {
    child: Child,
    staging: Option<(PathBuf, PathBuf)>,
}

impl Transfer {
    pub fn is_running(&mut self) -> Result<bool> {
        Ok(self.child.try_wait()?.is_none())
    }

    pub fn cancel(&mut self) -> Result<()> {
        if self.child.try_wait()?.is_none() {
            self.child.kill().context("cancel SFTP transfer")?;
        }
        let _ = self.child.wait();
        if let Some((staging, _)) = &self.staging {
            let _ = fs::remove_file(staging);
        }
        Ok(())
    }

    pub fn finish(mut self) -> Result<()> {
        let status = self.child.wait()?;
        ensure!(status.success(), "SFTP transfer failed");
        if let Some((staging, destination)) = self.staging.take() {
            ensure!(
                !destination.exists(),
                "local destination already exists; explicit overwrite is required"
            );
            fs::rename(&staging, &destination).context("commit downloaded file")?;
        }
        Ok(())
    }
}

impl Drop for Transfer {
    fn drop(&mut self) {
        if self.child.try_wait().ok().flatten().is_none() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
        if let Some((staging, _)) = &self.staging {
            let _ = fs::remove_file(staging);
        }
    }
}

fn spawn_batch(
    session: &Session,
    config: Option<&Path>,
    command: String,
) -> Result<Child> {
    let mut args = terminal::sftp_launch_args(session, config)?;
    args.insert(0, "-b".into());
    args.insert(1, "-".into());
    let mut child = Command::new("sftp")
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .context("start OpenSSH sftp transfer")?;
    writeln!(child.stdin.as_mut().context("open transfer stdin")?, "{command}")?;
    child.stdin.take();
    Ok(child)
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
    let parent = local.parent().unwrap_or_else(|| Path::new("."));
    let mut staging = tempfile::Builder::new()
        .prefix(".inspirum-download-")
        .tempfile_in(parent)
        .context("create download staging file")?;
    staging.as_file_mut().flush()?;
    let staging_path = staging.into_temp_path().keep()?;
    let command = format!(
        "get {} {}",
        quote_remote(remote)?,
        quote_remote(staging_path.to_str().context("local path must be valid Unicode")?)?
    );
    let child = spawn_batch(session, config, command)?;
    if overwrite && local.exists() {
        fs::remove_file(local).context("remove explicitly overwritten local file")?;
    }
    Ok(Transfer {
        child,
        staging: Some((staging_path, local.to_owned())),
    })
}

pub fn start_upload(
    session: &Session,
    config: Option<&Path>,
    local: &Path,
    remote: &str,
) -> Result<Transfer> {
    ensure!(local.is_file(), "upload source must be a regular file");
    let local = local.to_str().context("local path must be valid Unicode")?;
    let child = spawn_batch(
        session,
        config,
        format!("put {} {}", quote_remote(local)?, quote_remote(remote)?),
    )?;
    Ok(Transfer {
        child,
        staging: None,
    })
}

pub fn local_entries(path: &Path) -> Result<Vec<PathBuf>> {
    let mut entries = fs::read_dir(path)?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<std::io::Result<Vec<_>>>()?;
    entries.sort();
    Ok(entries)
}

pub fn reserve_local_destination(path: &Path) -> Result<()> {
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .context("local destination already exists; explicit overwrite is required")?;
    fs::remove_file(path)?;
    Ok(())
}
