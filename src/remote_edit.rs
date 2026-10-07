//! Safe internal text editing for remote SFTP files.
use crate::{Session, sftp};
use anyhow::{Context, Result, ensure};
use std::{
    fs,
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tempfile::TempDir;

const MAX_EDIT_BYTES: u64 = 1_048_576;
const TRANSFER_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SaveOutcome {
    Saved,
    Conflict,
}

pub struct RemoteEdit {
    remote: String,
    original: Vec<u8>,
    text: String,
    temp_dir: TempDir,
    working_path: PathBuf,
}

impl RemoteEdit {
    pub fn open(
        session: &Session,
        config: Option<&Path>,
        remote: &str,
        expected_size: Option<u64>,
    ) -> Result<Self> {
        if let Some(size) = expected_size {
            ensure!(size <= MAX_EDIT_BYTES, "remote file is too large for the text editor");
        }
        let temp_dir = private_temp_dir()?;
        let working_path = temp_dir.path().join("working.txt");
        download_blocking(session, config, remote, &working_path, expected_size)?;
        let original = fs::read(&working_path).context("read private remote edit working copy")?;
        ensure!(
            original.len() as u64 <= MAX_EDIT_BYTES,
            "remote file is too large for the text editor"
        );
        let text = decode_text(&original)?.to_owned();
        secure_file(&working_path)?;
        Ok(Self {
            remote: remote.to_owned(),
            original,
            text,
            temp_dir,
            working_path,
        })
    }

    pub fn remote(&self) -> &str {
        &self.remote
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn text_mut(&mut self) -> &mut String {
        &mut self.text
    }

    pub fn is_dirty(&self) -> bool {
        self.text.as_bytes() != self.original
    }

    pub fn working_path(&self) -> &Path {
        &self.working_path
    }

    pub fn save(
        &mut self,
        session: &Session,
        config: Option<&Path>,
        force_remote_change: bool,
    ) -> Result<SaveOutcome> {
        let current_path = self.temp_dir.path().join("conflict-check");
        let _ = fs::remove_file(&current_path);
        download_blocking(session, config, &self.remote, &current_path, None)?;
        let current = fs::read(&current_path).context("read remote conflict-check copy")?;
        let _ = fs::remove_file(&current_path);
        if !force_remote_change && current != self.original {
            return Ok(SaveOutcome::Conflict);
        }

        fs::write(&self.working_path, self.text.as_bytes())
            .context("write private remote edit working copy")?;
        secure_file(&self.working_path)?;

        let staged_remote = staging_remote_name(&self.remote)?;
        upload_blocking(session, config, &self.working_path, &staged_remote)?;
        if let Err(error) = sftp::rename_remote(session, config, &staged_remote, &self.remote) {
            let _ = sftp::delete_remote(session, config, &staged_remote, false);
            return Err(error).context("replace remote file from staged edit");
        }

        self.original = self.text.as_bytes().to_vec();
        Ok(SaveOutcome::Saved)
    }
}

fn decode_text(bytes: &[u8]) -> Result<&str> {
    ensure!(
        !bytes
            .iter()
            .any(|byte| (*byte < 0x20 && !matches!(*byte, b'\n' | b'\r' | b'\t')) || *byte == 0x7f),
        "remote file appears to be binary or contains unsupported control bytes"
    );
    std::str::from_utf8(bytes).context("remote file is not valid UTF-8 text")
}

fn private_temp_dir() -> Result<TempDir> {
    let dir = tempfile::Builder::new()
        .prefix(".inspirum-remote-edit-")
        .tempdir()
        .context("create private remote edit directory")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700))
            .context("restrict remote edit directory permissions")?;
    }
    Ok(dir)
}

fn secure_file(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))
            .context("restrict remote edit file permissions")?;
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
    Ok(())
}

fn wait_transfer(mut transfer: sftp::Transfer) -> Result<()> {
    let deadline = Instant::now() + TRANSFER_TIMEOUT;
    loop {
        match transfer.poll()? {
            Some(()) => return Ok(()),
            None => {
                ensure!(Instant::now() < deadline, "SFTP edit transfer timed out");
                thread::sleep(Duration::from_millis(20));
            }
        }
    }
}

fn download_blocking(
    session: &Session,
    config: Option<&Path>,
    remote: &str,
    destination: &Path,
    expected_size: Option<u64>,
) -> Result<()> {
    let transfer = sftp::start_download(
        session,
        config,
        remote,
        destination,
        true,
        expected_size,
    )?;
    wait_transfer(transfer)
}

fn upload_blocking(
    session: &Session,
    config: Option<&Path>,
    local: &Path,
    remote: &str,
) -> Result<()> {
    let transfer = sftp::start_upload(session, config, local, remote)?;
    wait_transfer(transfer)
}

fn staging_remote_name(remote: &str) -> Result<String> {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system clock is before Unix epoch")?
        .as_nanos();
    Ok(format!(
        "{remote}.inspirum-edit-{}-{stamp}",
        std::process::id()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_policy_accepts_utf8_and_rejects_binary_controls() {
        assert_eq!(decode_text("hello\n世界\t!\n".as_bytes()).unwrap(), "hello\n世界\t!\n");
        assert!(decode_text(&[0xff, 0xfe]).is_err());
        assert!(decode_text(b"hello\0world").is_err());
        assert!(decode_text(b"hello\x1bworld").is_err());
    }

    #[test]
    fn private_working_directory_is_cleaned_up_on_drop() {
        let dir = private_temp_dir().unwrap();
        let path = dir.path().to_owned();
        let file = path.join("working.txt");
        fs::write(&file, b"secret text").unwrap();
        secure_file(&file).unwrap();
        assert!(file.is_file());
        #[cfg(unix)]
        {
            use std::os::unix::fs::{MetadataExt, PermissionsExt};
            assert_eq!(fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o700);
            assert_eq!(fs::metadata(&file).unwrap().permissions().mode() & 0o777, 0o600);
            assert!(fs::metadata(&file).unwrap().uid() == fs::metadata(&path).unwrap().uid());
        }
        drop(dir);
        assert!(!path.exists());
    }

    #[test]
    fn dirty_state_is_byte_exact() {
        let dir = private_temp_dir().unwrap();
        let working_path = dir.path().join("working.txt");
        fs::write(&working_path, b"one\n").unwrap();
        let mut edit = RemoteEdit {
            remote: "file.txt".into(),
            original: b"one\n".to_vec(),
            text: "one\n".into(),
            temp_dir: dir,
            working_path,
        };
        assert!(!edit.is_dirty());
        edit.text_mut().push_str("two\n");
        assert!(edit.is_dirty());
    }
}
