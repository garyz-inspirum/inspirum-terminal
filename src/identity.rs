//! Private-key files that OpenSSH would refuse.
//!
//! OpenSSH ignores a private key whose mode is wider than `0600`
//! ("Permissions 0644 for '…' are too open … This private key will be
//! ignored") and there is no client option to override that. Keys kept in
//! cloud-synced folders (OneDrive, iCloud, Dropbox) routinely come back as
//! `0644` after every sync, so `chmod 600` is a fix that silently un-fixes
//! itself. Instead of handing OpenSSH the raw path, we hand it a private copy
//! kept under the application data directory with `0700`/`0600` permissions.
//! The original file is never modified.
//!
//! The copy is refreshed whenever the source's size or modification time
//! differs from the cached copy, so key rotation keeps working. Any failure
//! while copying falls back to the original path so OpenSSH reports the
//! real error rather than this module hiding it.

use std::{
    fs, io,
    path::{Path, PathBuf},
    time::SystemTime,
};

/// Directory under the per-user application data directory holding copies.
const CACHE_SUBDIR: &str = "keys";

/// Resolve the identity file to pass to `ssh -i`.
///
/// Returns the original path when it is already private (Unix mode `0600` or
/// tighter), does not exist, or cannot be protected; otherwise the path of a
/// private copy under the application data directory.
pub fn protected_identity_argument(identity_file: &str) -> String {
    match default_cache_dir() {
        Some(cache) => protected_identity_path(Path::new(identity_file), &cache)
            .ok()
            .and_then(|path| path.into_os_string().into_string().ok())
            .unwrap_or_else(|| identity_file.to_owned()),
        None => identity_file.to_owned(),
    }
}

/// Outcome of [`protected_identity_path`], useful for status messages.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Protection {
    /// The source already has private permissions; use it directly.
    Original,
    /// A fresh private copy was written.
    Copied,
    /// An up-to-date private copy already existed.
    Cached,
}

/// Like [`protected_identity_argument`] but with an explicit cache directory
/// and a report of what happened. Errors are returned, not swallowed.
pub fn protect_identity(source: &Path, cache_dir: &Path) -> io::Result<(PathBuf, Protection)> {
    let metadata = fs::metadata(source)?;
    if !metadata.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "identity file is not a regular file",
        ));
    }
    if !needs_copy(&metadata) {
        return Ok((source.to_path_buf(), Protection::Original));
    }

    let canonical = fs::canonicalize(source)?;
    let target = cache_dir.join(cache_name(&canonical));
    if copy_is_current(&target, &metadata) {
        return Ok((target, Protection::Cached));
    }

    ensure_private_dir(cache_dir)?;
    write_private_copy(&canonical, &target)?;
    // Mirror the source mtime so a later size+mtime comparison is meaningful.
    if let Ok(modified) = metadata.modified() {
        let _ = fs::File::options()
            .write(true)
            .open(&target)
            .and_then(|file| file.set_modified(modified));
    }
    Ok((target, Protection::Copied))
}

/// Path of the private copy for `source`, creating or refreshing it as needed.
pub fn protected_identity_path(source: &Path, cache_dir: &Path) -> io::Result<PathBuf> {
    protect_identity(source, cache_dir).map(|(path, _)| path)
}

fn default_cache_dir() -> Option<PathBuf> {
    directories::ProjectDirs::from("com", "Inspirum", "Inspirum Terminal")
        .map(|dirs| dirs.data_local_dir().join(CACHE_SUBDIR))
}

#[cfg(unix)]
fn needs_copy(metadata: &fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;
    metadata.permissions().mode() & 0o077 != 0
}

/// OpenSSH for Windows applies an ACL check we cannot inspect cheaply; a copy
/// restricted to the current user is always the safe choice.
#[cfg(not(unix))]
fn needs_copy(_metadata: &fs::Metadata) -> bool {
    true
}

fn copy_is_current(target: &Path, source: &fs::Metadata) -> bool {
    let Ok(existing) = fs::metadata(target) else {
        return false;
    };
    if !existing.is_file() || existing.len() != source.len() {
        return false;
    }
    if !private_enough(&existing) {
        return false;
    }
    match (existing.modified(), source.modified()) {
        (Ok(copy), Ok(original)) => same_second(copy, original),
        _ => false,
    }
}

fn same_second(left: SystemTime, right: SystemTime) -> bool {
    let secs = |time: SystemTime| {
        time.duration_since(SystemTime::UNIX_EPOCH)
            .map(|duration| duration.as_secs())
            .unwrap_or(0)
    };
    secs(left) == secs(right)
}

#[cfg(unix)]
fn private_enough(metadata: &fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;
    metadata.permissions().mode() & 0o077 == 0
}

#[cfg(not(unix))]
fn private_enough(_metadata: &fs::Metadata) -> bool {
    true
}

/// Stable, filesystem-safe name derived from the canonical source path
/// (FNV-1a 64-bit; no cryptographic strength needed, only stability).
fn cache_name(canonical: &Path) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in canonical.to_string_lossy().as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    let stem = canonical
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "identity".into());
    let stem: String = stem
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-'))
        .take(64)
        .collect();
    format!("{hash:016x}-{stem}")
}

fn ensure_private_dir(dir: &Path) -> io::Result<()> {
    fs::create_dir_all(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
    }
    #[cfg(windows)]
    restrict_acl(dir);
    Ok(())
}

fn write_private_copy(source: &Path, target: &Path) -> io::Result<()> {
    let bytes = fs::read(source)?;
    // Write to a sibling temp file with private permissions first, then rename,
    // so the key is never world-readable even for an instant.
    let temp = target.with_extension("tmp");
    {
        let mut options = fs::File::options();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temp)?;
        use std::io::Write;
        file.write_all(&bytes)?;
        file.sync_all()?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&temp, fs::Permissions::from_mode(0o600))?;
    }
    fs::rename(&temp, target)?;
    #[cfg(windows)]
    restrict_acl(target);
    Ok(())
}

/// Best effort: strip inherited ACEs and grant only the current user.
#[cfg(windows)]
fn restrict_acl(path: &Path) {
    let Ok(user) = std::env::var("USERNAME") else {
        return;
    };
    let _ = std::process::Command::new("icacls")
        .arg(path)
        .args(["/inheritance:r", "/grant:r"])
        .arg(format!("{user}:(F)"))
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn key(dir: &Path, name: &str, mode: u32) -> PathBuf {
        let path = dir.join(name);
        fs::write(&path, b"-----BEGIN OPENSSH PRIVATE KEY-----\nabc\n").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(mode)).unwrap();
        path
    }

    fn mode(path: &Path) -> u32 {
        fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    #[test]
    fn private_key_is_used_directly() {
        let dir = tempfile::tempdir().unwrap();
        let source = key(dir.path(), "id_ed25519", 0o600);
        let cache = dir.path().join("cache");
        let (path, outcome) = protect_identity(&source, &cache).unwrap();
        assert_eq!(path, source);
        assert_eq!(outcome, Protection::Original);
        assert!(
            !cache.exists(),
            "no cache directory for an already-private key"
        );
    }

    #[test]
    fn world_readable_key_is_copied_privately_and_source_untouched() {
        let dir = tempfile::tempdir().unwrap();
        let source = key(dir.path(), "id_ed25519", 0o644);
        let cache = dir.path().join("cache");
        let (copy, outcome) = protect_identity(&source, &cache).unwrap();
        assert_eq!(outcome, Protection::Copied);
        assert_ne!(copy, source);
        assert!(copy.starts_with(&cache));
        assert_eq!(mode(&copy), 0o600);
        assert_eq!(mode(&cache), 0o700);
        assert_eq!(mode(&source), 0o644, "original must not be modified");
        assert_eq!(fs::read(&copy).unwrap(), fs::read(&source).unwrap());
        assert!(
            !cache
                .join(copy.with_extension("tmp").file_name().unwrap())
                .exists()
        );
    }

    #[test]
    fn copy_is_reused_until_source_changes() {
        let dir = tempfile::tempdir().unwrap();
        let source = key(dir.path(), "id_ed25519", 0o644);
        let cache = dir.path().join("cache");
        let (first, _) = protect_identity(&source, &cache).unwrap();
        let (second, outcome) = protect_identity(&source, &cache).unwrap();
        assert_eq!(first, second, "stable cache path for the same source");
        assert_eq!(outcome, Protection::Cached);

        fs::write(&source, b"rotated key material that is longer\n").unwrap();
        fs::set_permissions(&source, fs::Permissions::from_mode(0o644)).unwrap();
        let (third, outcome) = protect_identity(&source, &cache).unwrap();
        assert_eq!(third, first);
        assert_eq!(outcome, Protection::Copied);
        assert_eq!(fs::read(&third).unwrap(), fs::read(&source).unwrap());
        assert_eq!(mode(&third), 0o600);
    }

    #[test]
    fn same_size_but_newer_source_is_recopied() {
        let dir = tempfile::tempdir().unwrap();
        let source = key(dir.path(), "id_ed25519", 0o644);
        let cache = dir.path().join("cache");
        let (copy, _) = protect_identity(&source, &cache).unwrap();
        fs::write(&source, b"-----BEGIN OPENSSH PRIVATE KEY-----\nxyz\n").unwrap();
        fs::set_permissions(&source, fs::Permissions::from_mode(0o644)).unwrap();
        let later = SystemTime::now() + std::time::Duration::from_secs(5);
        fs::File::options()
            .write(true)
            .open(&source)
            .unwrap()
            .set_modified(later)
            .unwrap();
        let (again, outcome) = protect_identity(&source, &cache).unwrap();
        assert_eq!(again, copy);
        assert_eq!(outcome, Protection::Copied);
        assert_eq!(fs::read(&again).unwrap(), fs::read(&source).unwrap());
    }

    #[test]
    fn different_sources_get_different_copies() {
        let dir = tempfile::tempdir().unwrap();
        let a = key(dir.path(), "id_a", 0o644);
        let b = key(dir.path(), "id_b", 0o644);
        let cache = dir.path().join("cache");
        let (copy_a, _) = protect_identity(&a, &cache).unwrap();
        let (copy_b, _) = protect_identity(&b, &cache).unwrap();
        assert_ne!(copy_a, copy_b);
    }

    #[test]
    fn missing_or_unreadable_key_falls_back_to_original_argument() {
        let missing = "/nonexistent/inspirum/id_ed25519";
        assert_eq!(protected_identity_argument(missing), missing);
        assert!(protect_identity(Path::new(missing), Path::new("/tmp")).is_err());
    }

    #[test]
    fn cache_name_is_stable_and_filesystem_safe() {
        let name = cache_name(Path::new(
            "/Users/g/OneDrive - Inspirum/Secure Keys/id_ed25519",
        ));
        assert_eq!(
            name,
            cache_name(Path::new(
                "/Users/g/OneDrive - Inspirum/Secure Keys/id_ed25519"
            ))
        );
        assert!(name.ends_with("-id_ed25519"));
        assert!(!name.contains(' ') && !name.contains('/'));
    }
}
