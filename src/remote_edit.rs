//! Safe internal text editing for remote SFTP files.
use crate::{Session, sftp};
use anyhow::{Context, Result, ensure};
use std::{
    fs,
    io::Read,
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
            ensure!(
                size <= MAX_EDIT_BYTES,
                "remote file is too large for the text editor"
            );
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
        if !force_remote_change
            && !remote_matches(
                session,
                config,
                &self.remote,
                self.temp_dir.path(),
                &self.original,
            )?
        {
            return Ok(SaveOutcome::Conflict);
        }

        fs::write(&self.working_path, self.text.as_bytes())
            .context("write private remote edit working copy")?;
        secure_file(&self.working_path)?;

        replace_remote_from(session, config, &self.working_path, &self.remote)?;

        self.original = self.text.as_bytes().to_vec();
        Ok(SaveOutcome::Saved)
    }
}

/// Byte-exact check that the remote file still equals `expected`.
///
/// Bounded so server-side growth cannot exhaust memory or disk:
/// 1. the remote size is queried first and any length mismatch is a conflict
///    (`Ok(false)`) without downloading;
/// 2. the download into `scratch_dir/conflict-check` is aborted (and its
///    staging file removed) as soon as it exceeds `expected.len()` bytes;
/// 3. the downloaded copy is compared in fixed-size chunks, never loaded whole.
///
/// Errors (missing remote, failed transfer, timeout) stay errors; they are
/// never reported as a match. Binary-safe: shared by the in-panel text editor
/// and the external-editor path.
pub(crate) fn remote_matches(
    session: &Session,
    config: Option<&Path>,
    remote: &str,
    scratch_dir: &Path,
    expected: &[u8],
) -> Result<bool> {
    let expected_len = expected.len() as u64;
    if sftp::remote_size(session, config, remote)? != expected_len {
        return Ok(false);
    }
    let current_path = scratch_dir.join("conflict-check");
    let _ = fs::remove_file(&current_path);
    let result = sftp::start_download(session, config, remote, &current_path, true, None)
        .and_then(|mut transfer| {
            wait_transfer_bounded(&mut transfer, expected_len, TRANSFER_TIMEOUT)
        })
        .and_then(|within_bound| {
            if within_bound {
                file_matches_bounded(&current_path, expected)
            } else {
                Ok(false)
            }
        });
    let _ = fs::remove_file(&current_path);
    result
}

const COMPARE_CHUNK_BYTES: usize = 64 * 1024;

/// Compare a local file with `expected` without loading it into memory.
/// Reads at most `expected.len() + 1` bytes even if the file is larger.
fn file_matches_bounded(path: &Path, expected: &[u8]) -> Result<bool> {
    let file = fs::File::open(path).context("open remote conflict-check copy")?;
    let actual_len = file
        .metadata()
        .context("stat remote conflict-check copy")?
        .len();
    if actual_len != expected.len() as u64 {
        return Ok(false);
    }
    reader_matches_bounded(file, expected)
}

/// Chunked comparison of `reader` against `expected`; consumes at most
/// `expected.len() + 1` bytes (the extra byte detects a longer stream).
fn reader_matches_bounded<R: Read>(reader: R, expected: &[u8]) -> Result<bool> {
    let mut limited = reader.take(expected.len() as u64 + 1);
    let mut buffer = vec![0_u8; COMPARE_CHUNK_BYTES];
    let mut offset = 0_usize;
    loop {
        let read = match limited.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => read,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error).context("read remote conflict-check copy"),
        };
        let end = offset + read;
        if end > expected.len() || buffer[..read] != expected[offset..end] {
            return Ok(false);
        }
        offset = end;
    }
    Ok(offset == expected.len())
}

/// Minimal transfer surface needed by the bounded wait loop (test seam).
trait BoundedTransfer {
    fn poll(&mut self) -> Result<Option<()>>;
    fn transferred_bytes(&self) -> u64;
    fn cancel(&mut self) -> Result<Option<PathBuf>>;
}

impl BoundedTransfer for sftp::Transfer {
    fn poll(&mut self) -> Result<Option<()>> {
        sftp::Transfer::poll(self)
    }

    fn transferred_bytes(&self) -> u64 {
        sftp::Transfer::transferred_bytes(self)
    }

    fn cancel(&mut self) -> Result<Option<PathBuf>> {
        sftp::Transfer::cancel(self)
    }
}

/// Wait for a download, aborting once more than `max_bytes` have arrived.
///
/// Returns `Ok(true)` when the transfer completed, `Ok(false)` when it was
/// cancelled for exceeding the bound (its preserved partial is deleted), and
/// `Err` on transfer failure or timeout. On error paths the transfer's own
/// staging file is removed when the transfer is dropped.
fn wait_transfer_bounded<T: BoundedTransfer>(
    transfer: &mut T,
    max_bytes: u64,
    timeout: Duration,
) -> Result<bool> {
    let deadline = Instant::now() + timeout;
    loop {
        if transfer.poll()?.is_some() {
            return Ok(true);
        }
        if transfer.transferred_bytes() > max_bytes {
            if let Ok(Some(partial)) = transfer.cancel() {
                let _ = fs::remove_file(partial);
            }
            return Ok(false);
        }
        ensure!(Instant::now() < deadline, "SFTP edit transfer timed out");
        thread::sleep(Duration::from_millis(20));
    }
}

/// Upload `local` to a staged remote name, then rename it over `remote`.
/// The staged file is deleted if the final rename fails.
pub(crate) fn replace_remote_from(
    session: &Session,
    config: Option<&Path>,
    local: &Path,
    remote: &str,
) -> Result<()> {
    let staged_remote = staging_remote_name(remote)?;
    upload_blocking(session, config, local, &staged_remote)?;
    if let Err(error) = sftp::rename_remote(session, config, &staged_remote, remote) {
        let _ = sftp::delete_remote(session, config, &staged_remote, false);
        return Err(error).context("replace remote file from staged edit");
    }
    Ok(())
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

pub(crate) fn private_temp_dir() -> Result<TempDir> {
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

pub(crate) fn secure_file(path: &Path) -> Result<()> {
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

pub(crate) fn download_blocking(
    session: &Session,
    config: Option<&Path>,
    remote: &str,
    destination: &Path,
    expected_size: Option<u64>,
) -> Result<()> {
    let transfer = sftp::start_download(session, config, remote, destination, true, expected_size)?;
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
        assert_eq!(
            decode_text("hello\n世界\t!\n".as_bytes()).unwrap(),
            "hello\n世界\t!\n"
        );
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
            assert_eq!(
                fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o700
            );
            assert_eq!(
                fs::metadata(&file).unwrap().permissions().mode() & 0o777,
                0o600
            );
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

    #[test]
    fn file_matches_bounded_compares_exact_bytes() {
        let dir = private_temp_dir().unwrap();
        let path = dir.path().join("current");
        fs::write(&path, b"hello world").unwrap();
        assert!(file_matches_bounded(&path, b"hello world").unwrap());
        assert!(!file_matches_bounded(&path, b"hello worlD").unwrap());
        assert!(!file_matches_bounded(&path, b"hello worl").unwrap());
        assert!(!file_matches_bounded(&path, b"hello world!").unwrap());
    }

    #[test]
    fn file_matches_bounded_rejects_much_larger_file() {
        let dir = private_temp_dir().unwrap();
        let path = dir.path().join("current");
        fs::write(&path, vec![b'a'; 1024 * 1024]).unwrap();
        assert!(!file_matches_bounded(&path, b"aaaaaaaaaa").unwrap());
    }

    #[test]
    fn file_matches_bounded_handles_empty_and_missing() {
        let dir = private_temp_dir().unwrap();
        let path = dir.path().join("current");
        fs::write(&path, b"").unwrap();
        assert!(file_matches_bounded(&path, b"").unwrap());
        assert!(!file_matches_bounded(&path, b"x").unwrap());
        assert!(file_matches_bounded(&dir.path().join("missing"), b"").is_err());
    }

    struct CountingReader<R> {
        inner: R,
        consumed: usize,
    }

    impl<R: Read> Read for CountingReader<R> {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            let read = self.inner.read(buf)?;
            self.consumed += read;
            Ok(read)
        }
    }

    #[test]
    fn reader_matches_bounded_reads_at_most_expected_plus_one() {
        let expected = b"aaaaaaaaaa";
        let mut reader = CountingReader {
            inner: std::io::Cursor::new(vec![b'a'; 1024 * 1024]),
            consumed: 0,
        };
        assert!(!reader_matches_bounded(&mut reader, expected).unwrap());
        assert!(reader.consumed <= expected.len() + 1);

        let mut exact = CountingReader {
            inner: std::io::Cursor::new(expected.to_vec()),
            consumed: 0,
        };
        assert!(reader_matches_bounded(&mut exact, expected).unwrap());
        assert_eq!(exact.consumed, expected.len());

        let mut longer = CountingReader {
            inner: std::io::Cursor::new(b"aaaaaaaaaab".to_vec()),
            consumed: 0,
        };
        assert!(!reader_matches_bounded(&mut longer, expected).unwrap());
        assert!(longer.consumed <= expected.len() + 1);

        let mut empty = CountingReader {
            inner: std::io::Cursor::new(Vec::new()),
            consumed: 0,
        };
        assert!(reader_matches_bounded(&mut empty, b"").unwrap());
    }

    struct FakeTransfer {
        polls_before_done: Option<usize>,
        bytes_per_poll: u64,
        bytes: u64,
        partial: PathBuf,
        cancelled: bool,
    }

    impl BoundedTransfer for FakeTransfer {
        fn poll(&mut self) -> Result<Option<()>> {
            self.bytes += self.bytes_per_poll;
            fs::write(&self.partial, vec![0_u8; self.bytes as usize]).unwrap();
            match self.polls_before_done {
                Some(0) => Ok(Some(())),
                Some(ref mut remaining) => {
                    *remaining -= 1;
                    Ok(None)
                }
                None => Ok(None),
            }
        }

        fn transferred_bytes(&self) -> u64 {
            self.bytes
        }

        fn cancel(&mut self) -> Result<Option<PathBuf>> {
            self.cancelled = true;
            Ok(Some(self.partial.clone()))
        }
    }

    fn fake_transfer(
        dir: &Path,
        polls_before_done: Option<usize>,
        bytes_per_poll: u64,
    ) -> FakeTransfer {
        FakeTransfer {
            polls_before_done,
            bytes_per_poll,
            bytes: 0,
            partial: dir.join(".inspirum-download-fake"),
            cancelled: false,
        }
    }

    #[test]
    fn bounded_wait_cancels_and_removes_partial_when_remote_grows() {
        let dir = private_temp_dir().unwrap();
        let mut transfer = fake_transfer(dir.path(), None, 4);
        let within = wait_transfer_bounded(&mut transfer, 10, Duration::from_secs(5)).unwrap();
        assert!(!within);
        assert!(transfer.cancelled);
        assert!((11..=12).contains(&transfer.bytes));
        assert!(!transfer.partial.exists());
    }

    #[test]
    fn bounded_wait_accepts_completed_transfer_within_bound() {
        let dir = private_temp_dir().unwrap();
        let mut transfer = fake_transfer(dir.path(), Some(2), 3);
        assert!(wait_transfer_bounded(&mut transfer, 10, Duration::from_secs(5)).unwrap());
        assert!(!transfer.cancelled);
    }

    #[test]
    fn bounded_wait_times_out_with_error() {
        let dir = private_temp_dir().unwrap();
        let mut transfer = fake_transfer(dir.path(), None, 0);
        assert!(wait_transfer_bounded(&mut transfer, 10, Duration::ZERO).is_err());
        assert!(!transfer.cancelled);
    }
}
