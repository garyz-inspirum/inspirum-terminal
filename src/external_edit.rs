//! Edit a remote SFTP file in an external desktop editor.
//!
//! Flow: download into a private per-edit directory (keeping the remote base
//! name so the OS picks the right editor), launch the configured or system
//! default editor detached, poll the working copy for real saves, and upload
//! them back with the same byte-exact remote-changed conflict check as the
//! in-panel editor (`remote_edit`). UI-free: the GUI drives it from
//! `Task::perform` plus a periodic tick.

use crate::{Session, remote_edit};
use anyhow::{Context, Result, bail, ensure};
use std::{
    ffi::{OsStr, OsString},
    fs, io,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::SystemTime,
};
use tempfile::TempDir;

/// External editors may open large or binary files; cap memory use anyway.
pub const MAX_EXTERNAL_EDIT_BYTES: u64 = 64 * 1024 * 1024;
const FILE_PLACEHOLDER: &str = "{file}";
const FALLBACK_NAME: &str = "remote-file";
const MAX_NAME_BYTES: usize = 200;

/// Which program opens the working copy.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EditorCommand {
    /// The OS file association (`explorer.exe` / `open` / `xdg-open`).
    SystemDefault,
    /// A user-configured program. `args` may contain `{file}`; if none does,
    /// the path is appended as the last argument.
    Custom { program: String, args: Vec<String> },
}

impl EditorCommand {
    /// Parse the user setting. Empty/whitespace means [`EditorCommand::SystemDefault`].
    ///
    /// Tokens are split on whitespace; double quotes group a token (so
    /// `"C:\Program Files\Notepad++\notepad++.exe" -multiInst` works) and
    /// backslashes are literal. There is no escape syntax and no shell.
    pub fn parse(setting: &str) -> Result<Self> {
        let mut tokens = tokenize(setting)?;
        if tokens.is_empty() {
            return Ok(Self::SystemDefault);
        }
        let program = tokens.remove(0);
        ensure!(!program.is_empty(), "editor program must not be empty");
        ensure!(
            !program.contains(FILE_PLACEHOLDER),
            "{FILE_PLACEHOLDER} cannot be used as the editor program"
        );
        Ok(Self::Custom {
            program,
            args: tokens,
        })
    }

    /// The exact program and argv that will be spawned for `path`.
    /// The path is always exactly one argv element (or embedded in one
    /// `{file}` token) and never passes through a shell.
    pub fn argv(&self, path: &Path) -> (OsString, Vec<OsString>) {
        match self {
            Self::SystemDefault => system_opener(path),
            Self::Custom { program, args } => {
                let mut out = Vec::with_capacity(args.len() + 1);
                let mut placed = false;
                for arg in args {
                    if arg.contains(FILE_PLACEHOLDER) {
                        placed = true;
                        out.push(substitute(arg, path.as_os_str()));
                    } else {
                        out.push(OsString::from(arg));
                    }
                }
                if !placed {
                    out.push(path.as_os_str().to_owned());
                }
                (OsString::from(program), out)
            }
        }
    }
}

fn tokenize(setting: &str) -> Result<Vec<String>> {
    ensure!(
        !setting.chars().any(|c| c.is_control() && c != '\t'),
        "editor command must not contain control characters"
    );
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut in_token = false;
    let mut quoted = false;
    for c in setting.chars() {
        match c {
            '"' => {
                quoted = !quoted;
                in_token = true;
            }
            c if c.is_whitespace() && !quoted => {
                if in_token {
                    tokens.push(std::mem::take(&mut current));
                    in_token = false;
                }
            }
            c => {
                current.push(c);
                in_token = true;
            }
        }
    }
    if quoted {
        bail!("editor command has an unterminated double quote");
    }
    if in_token {
        tokens.push(current);
    }
    Ok(tokens)
}

fn substitute(arg: &str, path: &OsStr) -> OsString {
    let mut out = OsString::new();
    let mut parts = arg.split(FILE_PLACEHOLDER);
    if let Some(first) = parts.next() {
        out.push(first);
    }
    for part in parts {
        out.push(path);
        out.push(part);
    }
    out
}

/// Platform opener for the system default application, as argv (no shell).
///
/// * Windows: `explorer.exe <path>`. `cmd /C start "" <path>` re-parses the
///   path as shell syntax (`&`, `^`, `%`), so it is avoided. `explorer.exe`
///   with an absolute file path opens it via its file association; std's argv
///   quoting keeps spaces intact. (`rundll32 url.dll,FileProtocolHandler`
///   receives the raw command-line tail and mishandles quoted paths, so it was
///   not chosen.) Note explorer.exe's exit code is not meaningful.
/// * macOS: `open <path>`.
/// * Linux/BSD: `xdg-open <path>`.
fn system_opener(path: &Path) -> (OsString, Vec<OsString>) {
    let program = if cfg!(windows) {
        "explorer.exe"
    } else if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    (OsString::from(program), vec![path.as_os_str().to_owned()])
}

/// Injectable process starter so tests never launch a real program.
pub type SpawnFn = fn(&OsStr, &[OsString]) -> Result<()>;

/// Real spawner: detached, null stdio, never waits for the editor in the
/// caller (many editors hand off to a running instance and exit at once).
/// A background thread reaps the child so no zombie is left on Unix.
pub fn spawn_detached(program: &OsStr, args: &[OsString]) -> Result<()> {
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .with_context(|| format!("start editor {}", Path::new(program).display()))?;
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

/// Open `path` with `command` using the real process spawner.
pub fn launch(command: &EditorCommand, path: &Path) -> Result<()> {
    launch_with(command, path, spawn_detached)
}

/// Open `path` with `command` using `spawn` (tests pass a recorder).
pub fn launch_with(command: &EditorCommand, path: &Path, spawn: SpawnFn) -> Result<()> {
    ensure!(path.is_file(), "working copy does not exist");
    let (program, args) = command.argv(path);
    spawn(&program, &args)
}

/// Turn a remote path into a safe local file name, keeping base name and
/// extension. Applied on every platform so names behave the same everywhere.
pub fn sanitize_file_name(remote: &str) -> String {
    let base = remote.rsplit(['/', '\\']).next().unwrap_or_default();
    let mut name: String = base
        .chars()
        .map(|c| {
            if c.is_control() || matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') {
                '_'
            } else {
                c
            }
        })
        .collect();
    while name.ends_with(['.', ' ']) {
        name.pop();
    }
    let trimmed = name.trim_start().to_owned();
    name = trimmed;
    if name.is_empty() || name == "." || name == ".." {
        return FALLBACK_NAME.to_owned();
    }
    let stem = name.split('.').next().unwrap_or_default();
    let upper = stem.trim_end().to_ascii_uppercase();
    let reserved = matches!(upper.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || ((upper.starts_with("COM") || upper.starts_with("LPT"))
            && upper.len() == 4
            && upper.as_bytes()[3].is_ascii_digit());
    if reserved {
        name.insert(0, '_');
    }
    if name.len() > MAX_NAME_BYTES {
        let ext = match name.rfind('.') {
            Some(dot) if dot > 0 && name.len() - dot <= 32 => name[dot..].to_owned(),
            _ => String::new(),
        };
        let mut end = MAX_NAME_BYTES - ext.len();
        while !name.is_char_boundary(end) {
            end -= 1;
        }
        name = format!("{}{ext}", &name[..end]);
    }
    name
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Stamp {
    len: u64,
    modified: Option<SystemTime>,
}

fn stamp(path: &Path) -> io::Result<Stamp> {
    let meta = fs::metadata(path)?;
    Ok(Stamp {
        len: meta.len(),
        modified: meta.modified().ok(),
    })
}

/// A detected local save whose content differs from the last synced bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Change {
    pub len: u64,
    pub modified: Option<SystemTime>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UploadOutcome {
    Uploaded,
    /// The remote file changed since download/last upload; retry with `force`.
    RemoteChangedConflict,
    /// Local content equals the last synced content; nothing sent.
    Unchanged,
    /// The working copy is currently missing (editor mid rename-save).
    WorkingCopyMissing,
}

/// One remote file being edited externally. Dropping it removes the private
/// per-edit directory (and only that directory).
pub struct ExternalEdit {
    remote: String,
    dir: TempDir,
    working_path: PathBuf,
    /// Bytes last known to equal the remote file (download or our upload).
    synced: Vec<u8>,
    synced_stamp: Option<Stamp>,
    /// Stamp seen on the previous poll, for the "two identical polls" debounce.
    pending: Option<Stamp>,
    /// Stamp already reported as a change, so it is reported once.
    reported: Option<Stamp>,
}

impl ExternalEdit {
    /// Download `remote` into a fresh private working copy.
    pub fn open(
        session: &Session,
        config: Option<&Path>,
        remote: &str,
        expected_size: Option<u64>,
    ) -> Result<Self> {
        if let Some(size) = expected_size {
            ensure!(
                size <= MAX_EXTERNAL_EDIT_BYTES,
                "remote file is too large for external editing"
            );
        }
        let (dir, working_path) = prepare_working_dir(remote)?;
        remote_edit::download_blocking(session, config, remote, &working_path, expected_size)?;
        Self::from_working_copy(remote, dir, working_path)
    }

    /// Adopt an already-downloaded working copy inside `dir`.
    fn from_working_copy(remote: &str, dir: TempDir, working_path: PathBuf) -> Result<Self> {
        ensure!(
            working_path.starts_with(dir.path()),
            "working copy must live in the private edit directory"
        );
        remote_edit::secure_file(&working_path)?;
        let synced = read_capped(&working_path)?;
        let synced_stamp = stamp(&working_path).ok();
        Ok(Self {
            remote: remote.to_owned(),
            dir,
            working_path,
            synced,
            synced_stamp,
            pending: None,
            reported: None,
        })
    }

    pub fn remote(&self) -> &str {
        &self.remote
    }

    pub fn working_path(&self) -> &Path {
        &self.working_path
    }

    /// Launch the editor on the working copy (real spawner).
    pub fn launch(&self, command: &EditorCommand) -> Result<()> {
        launch(command, &self.working_path)
    }

    /// Cheap tick check. Reports a change only when the file's size/mtime has
    /// been identical for two consecutive polls (not mid-write) and its bytes
    /// differ from the last synced content. A missing file (rename-style save
    /// in progress) is tolerated. Touch-only mtime changes are absorbed.
    pub fn poll_changed(&mut self) -> Result<Option<Change>> {
        let current = match stamp(&self.working_path) {
            Ok(current) => current,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                self.pending = None;
                return Ok(None);
            }
            Err(error) => return Err(error).context("inspect external edit working copy"),
        };
        if Some(current) == self.synced_stamp || Some(current) == self.reported {
            self.pending = None;
            return Ok(None);
        }
        if self.pending != Some(current) {
            self.pending = Some(current);
            return Ok(None);
        }
        self.pending = None;
        let bytes = match read_capped(&self.working_path) {
            Ok(bytes) => bytes,
            Err(_) if !self.working_path.exists() => return Ok(None),
            Err(error) => return Err(error),
        };
        if bytes == self.synced {
            self.synced_stamp = Some(current);
            return Ok(None);
        }
        self.reported = Some(current);
        Ok(Some(Change {
            len: current.len,
            modified: current.modified,
        }))
    }

    /// Upload the current working copy. Unless `force`, the remote file must
    /// still byte-equal the last synced content (same check as `RemoteEdit::save`).
    /// The bytes read here are snapshotted to a private staging file so a
    /// concurrent editor write cannot change what is uploaded.
    pub fn upload(
        &mut self,
        session: &Session,
        config: Option<&Path>,
        force: bool,
    ) -> Result<UploadOutcome> {
        let before = match stamp(&self.working_path) {
            Ok(before) => before,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Ok(UploadOutcome::WorkingCopyMissing);
            }
            Err(error) => return Err(error).context("inspect external edit working copy"),
        };
        let bytes = read_capped(&self.working_path)?;
        if bytes == self.synced {
            self.synced_stamp = Some(before);
            return Ok(UploadOutcome::Unchanged);
        }
        if !force
            && !remote_edit::remote_matches(
                session,
                config,
                &self.remote,
                self.dir.path(),
                &self.synced,
            )?
        {
            return Ok(UploadOutcome::RemoteChangedConflict);
        }
        let snapshot = self.dir.path().join("upload-snapshot");
        fs::write(&snapshot, &bytes).context("write external edit upload snapshot")?;
        remote_edit::secure_file(&snapshot)?;
        let result = remote_edit::replace_remote_from(session, config, &snapshot, &self.remote);
        let _ = fs::remove_file(&snapshot);
        result?;
        self.mark_synced(bytes, before);
        Ok(UploadOutcome::Uploaded)
    }

    fn mark_synced(&mut self, bytes: Vec<u8>, at: Stamp) {
        self.synced = bytes;
        self.synced_stamp = Some(at);
        self.pending = None;
        self.reported = None;
    }

    /// Remove the private edit directory now (also happens on drop).
    pub fn close(self) -> Result<()> {
        self.dir
            .close()
            .context("remove external edit working directory")
    }
}

/// Create `<private dir>/edit/<sanitised remote name>`. The `edit`
/// subdirectory holds only the user-visible file; internal scratch files
/// (conflict check, upload snapshot) live one level up.
fn prepare_working_dir(remote: &str) -> Result<(TempDir, PathBuf)> {
    let dir = remote_edit::private_temp_dir()?;
    let edit_dir = dir.path().join("edit");
    fs::create_dir(&edit_dir).context("create external edit directory")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&edit_dir, fs::Permissions::from_mode(0o700))
            .context("restrict external edit directory permissions")?;
    }
    let working_path = edit_dir.join(sanitize_file_name(remote));
    Ok((dir, working_path))
}

fn read_capped(path: &Path) -> Result<Vec<u8>> {
    use std::io::Read;
    let file = fs::File::open(path).context("open external edit working copy")?;
    let mut bytes = Vec::new();
    file.take(MAX_EXTERNAL_EDIT_BYTES + 1)
        .read_to_end(&mut bytes)
        .context("read external edit working copy")?;
    ensure!(
        bytes.len() as u64 <= MAX_EXTERNAL_EDIT_BYTES,
        "working copy is too large for external editing"
    );
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use std::time::Duration;

    fn custom(program: &str, args: &[&str]) -> EditorCommand {
        EditorCommand::Custom {
            program: program.into(),
            args: args.iter().map(|arg| (*arg).to_owned()).collect(),
        }
    }

    #[test]
    fn empty_setting_is_system_default() {
        assert_eq!(
            EditorCommand::parse("").unwrap(),
            EditorCommand::SystemDefault
        );
        assert_eq!(
            EditorCommand::parse("  \t ").unwrap(),
            EditorCommand::SystemDefault
        );
    }

    #[test]
    fn plain_and_quoted_programs_parse() {
        assert_eq!(
            EditorCommand::parse("code --wait").unwrap(),
            custom("code", &["--wait"])
        );
        assert_eq!(
            EditorCommand::parse(r#""C:\Program Files\Notepad++\notepad++.exe" -multiInst"#)
                .unwrap(),
            custom(r"C:\Program Files\Notepad++\notepad++.exe", &["-multiInst"])
        );
        assert!(EditorCommand::parse(r#""unterminated"#).is_err());
        assert!(EditorCommand::parse(r#""" -x"#).is_err());
        assert!(EditorCommand::parse("{file}").is_err());
    }

    #[test]
    fn path_is_appended_or_substituted_as_one_argument() {
        let path = Path::new("/tmp/x/a b \"q\" & c.txt");
        let (program, args) = custom("vim", &["-n"]).argv(path);
        assert_eq!(program, OsString::from("vim"));
        assert_eq!(
            args,
            vec![OsString::from("-n"), path.as_os_str().to_owned()]
        );

        let (_, args) = EditorCommand::parse("subl --file={file} -w")
            .unwrap()
            .argv(path);
        assert_eq!(
            args,
            vec![
                OsString::from("--file=/tmp/x/a b \"q\" & c.txt"),
                OsString::from("-w")
            ]
        );

        let (_, args) = EditorCommand::SystemDefault.argv(path);
        assert_eq!(args, vec![path.as_os_str().to_owned()]);
    }

    static SPAWNED: Mutex<Vec<(OsString, Vec<OsString>)>> = Mutex::new(Vec::new());

    fn record(program: &OsStr, args: &[OsString]) -> Result<()> {
        SPAWNED
            .lock()
            .unwrap()
            .push((program.to_owned(), args.to_vec()));
        Ok(())
    }

    #[test]
    fn launch_uses_injected_spawner() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("odd & name.txt");
        fs::write(&file, b"x").unwrap();
        launch_with(&custom("ed", &[]), &file, record).unwrap();
        let spawned = SPAWNED.lock().unwrap();
        let last = spawned.last().unwrap();
        assert_eq!(last.0, OsString::from("ed"));
        assert_eq!(last.1, vec![file.as_os_str().to_owned()]);
        drop(spawned);
        assert!(launch_with(&custom("ed", &[]), &dir.path().join("missing"), record).is_err());
    }

    #[test]
    fn names_are_sanitised_but_keep_extension() {
        assert_eq!(sanitize_file_name("/etc/nginx/nginx.conf"), "nginx.conf");
        assert_eq!(sanitize_file_name("notes.txt"), "notes.txt");
        assert_eq!(sanitize_file_name("/d/a:b?c*.log"), "a_b_c_.log");
        assert_eq!(sanitize_file_name("/d/trail. "), "trail");
        assert_eq!(sanitize_file_name("/d/CON.txt"), "_CON.txt");
        assert_eq!(sanitize_file_name("/d/com1"), "_com1");
        assert_eq!(sanitize_file_name("/d/console.txt"), "console.txt");
        assert_eq!(sanitize_file_name("/d/"), FALLBACK_NAME);
        assert_eq!(sanitize_file_name(".."), FALLBACK_NAME);
        let long = format!("/d/{}.yaml", "x".repeat(300));
        let name = sanitize_file_name(&long);
        assert!(name.len() <= MAX_NAME_BYTES);
        assert!(name.ends_with(".yaml"));
    }

    fn edit_with(content: &[u8]) -> ExternalEdit {
        let (dir, working) = prepare_working_dir("/srv/app/config file.txt").unwrap();
        fs::write(&working, content).unwrap();
        ExternalEdit::from_working_copy("/srv/app/config file.txt", dir, working).unwrap()
    }

    fn set_mtime(path: &Path, offset_secs: u64) {
        let file = fs::OpenOptions::new().write(true).open(path).unwrap();
        file.set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000 + offset_secs))
            .unwrap();
    }

    #[test]
    fn working_copy_keeps_name_in_private_dir() {
        let edit = edit_with(b"a");
        assert_eq!(
            edit.working_path().file_name().unwrap(),
            OsStr::new("config file.txt")
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let parent = edit.working_path().parent().unwrap();
            assert_eq!(
                fs::metadata(parent).unwrap().permissions().mode() & 0o777,
                0o700
            );
            assert_eq!(
                fs::metadata(edit.working_path())
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
    }

    #[test]
    fn no_change_and_touch_only_are_not_reported() {
        let mut edit = edit_with(b"same\n");
        assert_eq!(edit.poll_changed().unwrap(), None);
        assert_eq!(edit.poll_changed().unwrap(), None);
        set_mtime(edit.working_path(), 50);
        assert_eq!(edit.poll_changed().unwrap(), None);
        assert_eq!(edit.poll_changed().unwrap(), None);
        assert_eq!(edit.poll_changed().unwrap(), None);
    }

    #[test]
    fn real_edit_is_reported_once_after_debounce() {
        let mut edit = edit_with(b"one\n");
        fs::write(edit.working_path(), b"two\n").unwrap();
        set_mtime(edit.working_path(), 10);
        assert_eq!(
            edit.poll_changed().unwrap(),
            None,
            "first sighting debounces"
        );
        let change = edit
            .poll_changed()
            .unwrap()
            .expect("stable edit is reported");
        assert_eq!(change.len, 4);
        assert_eq!(edit.poll_changed().unwrap(), None, "reported only once");
    }

    #[test]
    fn mid_write_growth_waits_for_stable_size() {
        let mut edit = edit_with(b"a");
        fs::write(edit.working_path(), b"ab").unwrap();
        set_mtime(edit.working_path(), 1);
        assert_eq!(edit.poll_changed().unwrap(), None);
        fs::write(edit.working_path(), b"abc").unwrap();
        set_mtime(edit.working_path(), 2);
        assert_eq!(edit.poll_changed().unwrap(), None, "still changing");
        assert!(edit.poll_changed().unwrap().is_some());
    }

    #[test]
    fn rename_style_save_tolerates_missing_file() {
        let mut edit = edit_with(b"old");
        let path = edit.working_path().to_owned();
        let tmp = path.with_file_name(".config file.txt.swp");
        fs::write(&tmp, b"new content").unwrap();
        fs::remove_file(&path).unwrap();
        assert_eq!(edit.poll_changed().unwrap(), None);
        assert_eq!(edit.poll_changed().unwrap(), None);
        fs::rename(&tmp, &path).unwrap();
        set_mtime(&path, 5);
        assert_eq!(edit.poll_changed().unwrap(), None);
        assert!(edit.poll_changed().unwrap().is_some());
    }

    #[test]
    fn binary_content_is_compared_byte_exact() {
        let mut edit = edit_with(&[0xff, 0x00, 0xfe]);
        fs::write(edit.working_path(), [0xff, 0x00, 0xfd]).unwrap();
        set_mtime(edit.working_path(), 3);
        assert_eq!(edit.poll_changed().unwrap(), None);
        assert!(edit.poll_changed().unwrap().is_some());
    }

    #[test]
    fn mark_synced_advances_baseline() {
        let mut edit = edit_with(b"v1");
        fs::write(edit.working_path(), b"v2").unwrap();
        set_mtime(edit.working_path(), 7);
        let at = stamp(edit.working_path()).unwrap();
        edit.mark_synced(b"v2".to_vec(), at);
        assert_eq!(edit.poll_changed().unwrap(), None);
        assert_eq!(edit.poll_changed().unwrap(), None);
    }

    #[test]
    fn cleanup_is_confined_to_private_directory() {
        let outside = tempfile::tempdir().unwrap();
        let sentinel = outside.path().join("keep.txt");
        fs::write(&sentinel, b"keep").unwrap();

        let edit = edit_with(b"x");
        let root = edit.dir.path().to_owned();
        assert!(edit.working_path().starts_with(&root));
        edit.close().unwrap();
        assert!(!root.exists());
        assert!(sentinel.is_file());

        let dropped = edit_with(b"y");
        let root = dropped.dir.path().to_owned();
        drop(dropped);
        assert!(!root.exists());
        assert!(sentinel.is_file());

        let (dir, _) = prepare_working_dir("/a/b.txt").unwrap();
        let stray = outside.path().join("b.txt");
        fs::write(&stray, b"z").unwrap();
        assert!(ExternalEdit::from_working_copy("/a/b.txt", dir, stray.clone()).is_err());
        assert!(stray.is_file());
    }
}
