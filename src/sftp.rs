//! Machine-readable SFTP operations backed by the system OpenSSH sftp client.
use crate::{Session, terminal};
use anyhow::{Context, Result, ensure};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Child, Command, ExitStatus, Stdio},
};
use tempfile::TempPath;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemoteEntry {
    pub name: String,
    pub is_dir: bool,
    pub size: Option<u64>,
}

fn quote_batch_arg(value: &str) -> Result<String> {
    ensure!(!value.is_empty(), "SFTP path is required");
    ensure!(
        !value.chars().any(char::is_control),
        "SFTP path contains control characters"
    );
    Ok(format!(
        "\"{}\"",
        value.replace('\\', "\\\\").replace('"', "\\\"")
    ))
}

fn batch_args(session: &Session, config: Option<&Path>) -> Result<Vec<String>> {
    let mut args = terminal::sftp_launch_args(session, config)?;
    args.splice(0..0, ["-b".into(), "-".into()]);
    Ok(args)
}

fn run_batch(session: &Session, config: Option<&Path>, commands: &[String]) -> Result<String> {
    let mut child = Command::new("sftp")
        .args(batch_args(session, config)?)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("start OpenSSH sftp batch operation")?;
    {
        let mut stdin = child.stdin.take().context("open sftp batch stdin")?;
        for command in commands {
            writeln!(stdin, "{command}")?;
        }
    }
    let output = child.wait_with_output()?;
    ensure_sftp_success(output.status, &output.stderr)?;
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn ensure_sftp_success(status: ExitStatus, stderr: &[u8]) -> Result<()> {
    ensure!(
        status.success(),
        "SFTP operation failed: {}",
        String::from_utf8_lossy(stderr).trim()
    );
    Ok(())
}

fn parse_listing(output: &str) -> Vec<RemoteEntry> {
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
        let mut name = fields[8..].join(" ");
        if fields[0].starts_with('l')
            && let Some((link_name, _)) = name.split_once(" -> ")
        {
            name = link_name.to_owned();
        }
        if name == "." || name == ".." {
            continue;
        }
        entries.push(RemoteEntry {
            name,
            is_dir: fields[0].starts_with('d'),
            size: fields[4].parse().ok(),
        });
    }
    entries
}

pub fn list_remote(
    session: &Session,
    config: Option<&Path>,
    remote_dir: &str,
) -> Result<Vec<RemoteEntry>> {
    let output = run_batch(
        session,
        config,
        &[
            format!("cd {}", quote_batch_arg(remote_dir)?),
            "ls -lan".into(),
        ],
    )?;
    Ok(parse_listing(&output))
}

pub fn remote_size(session: &Session, config: Option<&Path>, remote: &str) -> Result<u64> {
    let output = run_batch(
        session,
        config,
        &[format!("ls -lan {}", quote_batch_arg(remote)?)],
    )?;
    parse_listing(&output)
        .into_iter()
        .find_map(|entry| entry.size)
        .context("SFTP listing did not report remote file size")
}

pub fn mkdir_remote(session: &Session, config: Option<&Path>, path: &str) -> Result<()> {
    run_batch(
        session,
        config,
        &[format!("mkdir {}", quote_batch_arg(path)?)],
    )?;
    Ok(())
}

pub fn rename_remote(session: &Session, config: Option<&Path>, from: &str, to: &str) -> Result<()> {
    run_batch(
        session,
        config,
        &[format!(
            "rename {} {}",
            quote_batch_arg(from)?,
            quote_batch_arg(to)?
        )],
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
        &[format!("{command} {}", quote_batch_arg(path)?)],
    )?;
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TransferKind {
    Download,
    Upload,
}

pub struct Transfer {
    child: Child,
    kind: TransferKind,
    staging: Option<TempPath>,
    destination: Option<PathBuf>,
    overwrite: bool,
    expected_size: Option<u64>,
    session: Session,
    config: Option<PathBuf>,
    remote: String,
}

impl Transfer {
    pub fn is_running(&mut self) -> Result<bool> {
        Ok(self.child.try_wait()?.is_none())
    }

    pub fn transferred_bytes(&self) -> u64 {
        match self.kind {
            TransferKind::Download => self
                .staging
                .as_ref()
                .and_then(|path| fs::metadata(path).ok())
                .map(|metadata| metadata.len())
                .unwrap_or(0),
            TransferKind::Upload => 0,
        }
    }

    pub fn expected_bytes(&self) -> Option<u64> {
        self.expected_size
    }

    pub fn cancel(&mut self) -> Result<()> {
        if self.child.try_wait()?.is_none() {
            self.child.kill().context("cancel SFTP transfer")?;
        }
        let _ = self.child.wait();
        self.staging.take();
        Ok(())
    }

    fn finalize_success(&mut self) -> Result<()> {
        match self.kind {
            TransferKind::Download => {
                let staging = self
                    .staging
                    .take()
                    .context("download staging file is missing")?;
                let actual = fs::metadata(&staging)?.len();
                if let Some(expected) = self.expected_size {
                    ensure!(
                        actual == expected,
                        "download integrity check failed: expected {expected} bytes, received {actual}"
                    );
                }
                let destination = self
                    .destination
                    .as_ref()
                    .context("download destination is missing")?;
                if self.overwrite {
                    staging
                        .persist(destination)
                        .map_err(|error| error.error)
                        .context("commit verified downloaded file")?;
                } else {
                    staging
                        .persist_noclobber(destination)
                        .map_err(|error| error.error)
                        .context(
                            "commit verified downloaded file without overwriting an existing destination",
                        )?;
                }
            }
            TransferKind::Upload => {
                let expected = self
                    .expected_size
                    .context("upload source size is missing")?;
                let actual = remote_size(&self.session, self.config.as_deref(), &self.remote)?;
                ensure!(
                    actual == expected,
                    "upload integrity check failed: expected {expected} bytes remotely, found {actual}"
                );
            }
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
        ensure_sftp_success(status, &stderr)?;
        self.finalize_success()?;
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
    }
}

fn spawn_batch(session: &Session, config: Option<&Path>, command: String) -> Result<Child> {
    let mut child = Command::new("sftp")
        .args(batch_args(session, config)?)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .context("start OpenSSH sftp transfer")?;
    {
        let mut stdin = child.stdin.take().context("open transfer stdin")?;
        writeln!(stdin, "{command}")?;
    }
    Ok(child)
}

pub fn start_download(
    session: &Session,
    config: Option<&Path>,
    remote: &str,
    local: &Path,
    overwrite: bool,
    expected_size: Option<u64>,
) -> Result<Transfer> {
    if local.exists() && !overwrite {
        anyhow::bail!("local destination already exists; explicit overwrite is required");
    }
    let parent = local.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent).context("create local download directory")?;
    let staging = tempfile::Builder::new()
        .prefix(".inspirum-download-")
        .tempfile_in(parent)
        .context("create download staging file")?
        .into_temp_path();
    let command = format!(
        "get {} {}",
        quote_batch_arg(remote)?,
        quote_batch_arg(
            staging
                .to_str()
                .context("local staging path must be valid Unicode")?
        )?
    );
    let child = spawn_batch(session, config, command)?;
    Ok(Transfer {
        child,
        kind: TransferKind::Download,
        staging: Some(staging),
        destination: Some(local.to_owned()),
        overwrite,
        expected_size,
        session: session.clone(),
        config: config.map(Path::to_owned),
        remote: remote.to_owned(),
    })
}

pub fn start_upload(
    session: &Session,
    config: Option<&Path>,
    local: &Path,
    remote: &str,
) -> Result<Transfer> {
    ensure!(local.is_file(), "upload source must be a regular file");
    let expected_size = fs::metadata(local)?.len();
    let local_text = local.to_str().context("local path must be valid Unicode")?;
    let child = spawn_batch(
        session,
        config,
        format!(
            "put {} {}",
            quote_batch_arg(local_text)?,
            quote_batch_arg(remote)?
        ),
    )?;
    Ok(Transfer {
        child,
        kind: TransferKind::Upload,
        staging: None,
        destination: None,
        overwrite: false,
        expected_size: Some(expected_size),
        session: session.clone(),
        config: config.map(Path::to_owned),
        remote: remote.to_owned(),
    })
}

pub fn local_entries(path: &Path) -> Result<Vec<PathBuf>> {
    let mut entries = fs::read_dir(path)?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<std::io::Result<Vec<_>>>()?;
    entries.sort_by(|left, right| {
        let left_dir = left.is_dir();
        let right_dir = right.is_dir();
        right_dir
            .cmp(&left_dir)
            .then_with(|| left.file_name().cmp(&right.file_name()))
    });
    Ok(entries)
}

pub fn join_remote(directory: &str, name: &str) -> String {
    let directory = directory.trim_end_matches('/');
    if directory.is_empty() || directory == "/" {
        format!("/{name}")
    } else if directory == "." {
        name.to_owned()
    } else {
        format!("{directory}/{name}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_join_and_listing_parser_handle_spaces() {
        assert_eq!(join_remote("/", "hello world"), "/hello world");
        assert_eq!(join_remote("/tmp", "hello world"), "/tmp/hello world");
        let parsed = parse_listing(
            "-rw-r--r-- 1 1000 1000 12 Jan 01 00:00 hello world.txt\n\
             drwxr-xr-x 2 1000 1000 4096 Jan 01 00:00 folder name\n",
        );
        assert_eq!(
            parsed,
            vec![
                RemoteEntry {
                    name: "hello world.txt".into(),
                    is_dir: false,
                    size: Some(12),
                },
                RemoteEntry {
                    name: "folder name".into(),
                    is_dir: true,
                    size: Some(4096),
                }
            ]
        );
    }

    #[test]
    fn batch_quoting_rejects_control_characters() {
        assert!(quote_batch_arg("normal path").is_ok());
        assert!(quote_batch_arg("bad\npath").is_err());
    }
}
