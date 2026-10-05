//! Local-only support diagnostics. Never evaluate SSH config or collect session payloads.
use inspirum_terminal::{Session, load_sessions};
use std::{
    fs::File,
    io::{Read, Seek, SeekFrom, Write},
    path::Path,
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

const OUTPUT_LIMIT: u64 = 65_536;
const PROBE_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Debug, PartialEq, Eq)]
enum ProbeFailure {
    Missing,
    Launch,
    Io,
    Timeout,
    OutputLimit,
}

impl ProbeFailure {
    fn label(&self) -> &'static str {
        match self {
            Self::Missing => "not found on PATH",
            Self::Launch => "could not start",
            Self::Io => "local probe I/O failed",
            Self::Timeout => "local probe timed out",
            Self::OutputLimit => "local probe exceeded output limit",
        }
    }
}

struct Capture {
    success: bool,
    stdout: String,
    stderr: String,
}

struct ReapedChild(Child);

impl Drop for ReapedChild {
    fn drop(&mut self) {
        // Also runs after a timeout, output-limit error, or early I/O failure.
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn read_capture(file: &mut File) -> Result<String, ProbeFailure> {
    file.seek(SeekFrom::Start(0))
        .map_err(|_| ProbeFailure::Io)?;
    let mut bytes = Vec::new();
    file.take(OUTPUT_LIMIT + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| ProbeFailure::Io)?;
    if bytes.len() as u64 > OUTPUT_LIMIT {
        return Err(ProbeFailure::OutputLimit);
    }
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

fn probe(program: &Path, args: &[&str], timeout: Duration) -> Result<Capture, ProbeFailure> {
    // Anonymous temporary files avoid pipe deadlocks and blocked reader threads. Polling
    // limits capture growth; only at most OUTPUT_LIMIT bytes can enter the report parser.
    let mut stdout = tempfile::tempfile().map_err(|_| ProbeFailure::Io)?;
    let mut stderr = tempfile::tempfile().map_err(|_| ProbeFailure::Io)?;
    let child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::from(
            stdout.try_clone().map_err(|_| ProbeFailure::Io)?,
        ))
        .stderr(Stdio::from(
            stderr.try_clone().map_err(|_| ProbeFailure::Io)?,
        ))
        .spawn()
        .map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                ProbeFailure::Missing
            } else {
                ProbeFailure::Launch
            }
        })?;
    let mut child = ReapedChild(child);
    let started = Instant::now();
    let status = loop {
        let size = stdout.metadata().map_err(|_| ProbeFailure::Io)?.len()
            + stderr.metadata().map_err(|_| ProbeFailure::Io)?.len();
        if size > OUTPUT_LIMIT {
            return Err(ProbeFailure::OutputLimit);
        }
        if let Some(status) = child.0.try_wait().map_err(|_| ProbeFailure::Io)? {
            break status;
        }
        if started.elapsed() >= timeout {
            return Err(ProbeFailure::Timeout);
        }
        thread::sleep(Duration::from_millis(10));
    };
    let out = read_capture(&mut stdout)?;
    let err = read_capture(&mut stderr)?;
    if out.len() + err.len() > OUTPUT_LIMIT as usize {
        return Err(ProbeFailure::OutputLimit);
    }
    Ok(Capture {
        success: status.success(),
        stdout: out,
        stderr: err,
    })
}

fn openssh_version(text: &str) -> Option<String> {
    for token in text.split_whitespace() {
        let version = token
            .strip_prefix("OpenSSH_for_Windows_")
            .or_else(|| token.strip_prefix("OpenSSH_"));
        let Some(version) = version else {
            continue;
        };
        let version = version.trim_end_matches(',');
        if version.len() > 32 {
            continue;
        }
        let (release, patch) = version.split_once('p').unwrap_or((version, "0"));
        let Some((major, minor)) = release.split_once('.') else {
            continue;
        };
        if [major, minor, patch]
            .iter()
            .all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()))
        {
            return Some(version.to_owned());
        }
    }
    None
}

fn describe_version(result: Result<Capture, ProbeFailure>) -> String {
    match result {
        Ok(capture) if capture.success => openssh_version(&capture.stdout)
            .or_else(|| openssh_version(&capture.stderr))
            .map(|version| format!("OpenSSH {version}"))
            .unwrap_or_else(|| "unrecognized version response (raw output omitted)".into()),
        Ok(_) => "version probe failed (raw output omitted)".into(),
        Err(error) => error.label().into(),
    }
}

fn describe_usage(result: Result<Capture, ProbeFailure>, expected: &str) -> String {
    match result {
        Ok(capture) if capture.stdout.contains(expected) || capture.stderr.contains(expected) => {
            "available (usage response; not a connectivity test)".into()
        }
        Ok(_) => "unrecognized usage response (raw output omitted)".into(),
        Err(error) => error.label().into(),
    }
}

fn policy(value: Option<bool>) -> &'static str {
    match value {
        None => "inherit",
        Some(true) => "enabled",
        Some(false) => "disabled",
    }
}

fn profile_summary(session: &Session) -> String {
    // Never serialize a Session or render its strings. Even remote commands may contain
    // user-entered secrets, so allowlist booleans and counts instead of trying to redact.
    format!(
        "Saved profile: selected (identifiers omitted)\n\
         Host trust: {}\n\
         Username override: {}\n\
         Port override: {}\n\
         Identity path configured: {}\n\
         Jump route configured: {}\n\
         Structured proxy: {} (endpoint omitted)\n\
         Remote command configured: {} (contents omitted)\n\
         Forward counts: local={}, remote={}, dynamic={}\n\
         Authentication: public-key={}, password={}, keyboard-interactive={}\n\
         GSSAPI: authentication={}, delegation={}\n\
         IdentitiesOnly: {}\n\
         Terminal agent forwarding: {}\n\
         Terminal X11 forwarding: {}\n\
         Compression: {}\n",
        if session.strict {
            "already trusted only"
        } else {
            "ask before new trust"
        },
        !session.user.is_empty(),
        session.port.is_some(),
        !session.ssh.identity_file.is_empty(),
        !session.ssh.proxy_jump.is_empty(),
        match session.ssh.proxy_kind {
            inspirum_terminal::ProxyKind::None => "none",
            inspirum_terminal::ProxyKind::HttpConnect => "http-connect",
            inspirum_terminal::ProxyKind::Socks5 => "socks5",
        },
        !session.ssh.remote_command.is_empty(),
        session.ssh.local_forwards.len(),
        session.ssh.remote_forwards.len(),
        session.ssh.dynamic_forwards.len(),
        policy(session.ssh.public_key_auth),
        policy(session.ssh.password_auth),
        policy(session.ssh.keyboard_interactive_auth),
        policy(session.ssh.gssapi_auth),
        policy(session.ssh.gssapi_delegate_credentials),
        policy(session.ssh.identities_only),
        policy(session.ssh.agent_forwarding),
        policy(session.ssh.x11_forwarding),
        policy(session.ssh.compression),
    )
}

pub fn collect(profiles: Option<&Path>, selected: Option<&str>, explicit_config: bool) -> String {
    let version = describe_version(probe(Path::new("ssh"), &["-V"], PROBE_TIMEOUT));
    let sftp = describe_usage(
        probe(Path::new("sftp"), &["-h"], PROBE_TIMEOUT),
        "usage: sftp",
    );
    let keygen = describe_usage(
        probe(Path::new("ssh-keygen"), &["-?"], PROBE_TIMEOUT),
        "usage: ssh-keygen",
    );
    let queries = match probe(Path::new("ssh"), &["-Q", "help"], PROBE_TIMEOUT) {
        Ok(capture) if capture.success => {
            let has = |query: &str| capture.stdout.lines().any(|line| line.trim() == query);
            format!(
                "cipher={}, kex={}, key={}, mac={} (local query support only)",
                has("cipher"),
                has("kex"),
                has("key"),
                has("mac")
            )
        }
        Ok(_) => "not available (raw output omitted)".into(),
        Err(error) => error.label().into(),
    };
    let mut report = format!(
        "Inspirum Terminal support report - schema 1\n\
         Application: {}\n\
         Platform: {} / {}\n\
         Collection: local probes only; no SSH connection or config evaluation\n\
         SSH config source: {} (path and contents omitted)\n\
         SSH: {version}\nSFTP: {sftp}\nssh-keygen: {keygen}\n\
         SSH query support: {queries}\n",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        std::env::consts::ARCH,
        if explicit_config {
            "explicit"
        } else {
            "default"
        },
    );
    if let (Some(path), Some(name)) = (profiles, selected) {
        match load_sessions(path) {
            Ok(sessions) => {
                let matches: Vec<_> = sessions.iter().filter(|s| s.name == name).collect();
                if matches.len() == 1 {
                    report.push_str(&profile_summary(matches[0]));
                } else {
                    report
                        .push_str("Saved profile: not found or ambiguous (identifiers omitted)\n");
                }
            }
            Err(_) => report.push_str("Saved profiles: unavailable or invalid (details omitted)\n"),
        }
    } else {
        report.push_str("Saved profile: not requested; profile store not read\n");
    }
    report.push_str(
        "Excluded: hostnames, usernames, profile names, paths, keys, credentials,\n\
         remote commands, environment variables, terminal contents and raw error output.\n\
         This report does not establish SSH connectivity or authentication support.\n",
    );
    report
}

pub fn export(path: &Path, report: &str) -> anyhow::Result<()> {
    anyhow::ensure!(
        report.len() <= OUTPUT_LIMIT as usize,
        "support report is too large"
    );
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let mut file = tempfile::NamedTempFile::new_in(parent)
        .map_err(|_| anyhow::anyhow!("cannot create support report in destination directory"))?;
    file.write_all(report.as_bytes())
        .and_then(|_| file.as_file().sync_all())
        .map_err(|_| anyhow::anyhow!("cannot write support report"))?;
    file.persist_noclobber(path)
        .map_err(|_| anyhow::anyhow!("support report destination exists or is not writable"))?;
    Ok(())
}

#[cfg(test)]
mod tests;
