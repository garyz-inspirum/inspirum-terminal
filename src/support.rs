//! Privacy-safe local diagnostics and support-bundle rendering.
use crate::Session;
use anyhow::Result;
use std::{
    collections::VecDeque,
    fs::File,
    io::{Read, Seek, SeekFrom, Write},
    path::Path,
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

const OUTPUT_LIMIT: u64 = 65_536;
const PROBE_TIMEOUT: Duration = Duration::from_secs(2);
pub const MAX_RECENT_ERRORS: usize = 12;

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
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SanitizedErrorHistory {
    entries: VecDeque<&'static str>,
}

impl SanitizedErrorHistory {
    pub fn record(&mut self, error: &str) {
        if error.trim().is_empty() {
            return;
        }
        if self.entries.len() == MAX_RECENT_ERRORS {
            self.entries.pop_front();
        }
        self.entries.push_back(classify_error(error));
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    fn render(&self) -> String {
        if self.entries.is_empty() {
            return "Recent sanitized connection errors: none recorded in this app run\n".into();
        }
        let mut text = format!(
            "Recent sanitized connection errors: {} (in-memory, oldest to newest)\n",
            self.entries.len()
        );
        for (index, entry) in self.entries.iter().enumerate() {
            text.push_str(&format!("  {}. {entry}\n", index + 1));
        }
        text
    }
}

fn classify_error(error: &str) -> &'static str {
    let lower = error.to_ascii_lowercase();
    if lower.contains("host key")
        || lower.contains("known_hosts")
        || lower.contains("remote host identification has changed")
    {
        "host-key trust or verification failure"
    } else if lower.contains("permission denied")
        || lower.contains("authentication")
        || lower.contains("password")
        || lower.contains("passphrase")
        || lower.contains("keyboard-interactive")
        || lower.contains("gssapi")
    {
        "authentication failure"
    } else if lower.contains("timed out") || lower.contains("timeout") {
        "connection timeout"
    } else if lower.contains("connection refused") {
        "connection refused"
    } else if lower.contains("no route") || lower.contains("network is unreachable") {
        "network unreachable"
    } else if lower.contains("resolve hostname")
        || lower.contains("name or service not known")
        || lower.contains("nodename nor servname")
    {
        "name resolution failure"
    } else if lower.contains("proxy") || lower.contains("socks") || lower.contains("http connect") {
        "proxy transport failure"
    } else if lower.contains("controlmaster") || lower.contains("controlpath") {
        "SSH multiplexing failure"
    } else if lower.contains("tmux") {
        "tmux session operation failure"
    } else if lower.contains("sftp") || lower.contains("scp") || lower.contains("transfer") {
        "file-transfer operation failure"
    } else if lower.contains("tunnel") || lower.contains("forward") || lower.contains("listener") {
        "SSH forwarding failure"
    } else if lower.contains("openssh") || lower.contains("ssh client unavailable") {
        "local OpenSSH availability failure"
    } else if lower.contains("pty") || lower.contains("terminal") {
        "terminal or PTY operation failure"
    } else {
        "connection operation failure"
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

pub fn policy_summary(session: &Session) -> String {
    format!(
        "Effective app launch policy: selected profile/draft (identifiers omitted)\n\
         Host trust: {}\n\
         Username override configured: {}\n\
         Port override configured: {}\n\
         Explicit identity path configured: {}\n\
         ProxyJump configured: {}\n\
         Structured proxy: {} (endpoint omitted)\n\
         ControlMaster: {} (path omitted, persist configured={})\n\
         Remote command configured: {} (contents omitted)\n\
         Forward counts: local={}, remote={}, dynamic={}\n\
         Authentication: public-key={}, password={}, keyboard-interactive={}\n\
         GSSAPI: authentication={}, delegation={}\n\
         IdentitiesOnly: {}\n\
         Terminal agent forwarding: {}\n\
         Terminal X11 forwarding: {}\n\
         Compression: {}\n\
         Connect timeout configured: {}\n\
         Server keepalive configured: {}\n",
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
            crate::ProxyKind::None => "none",
            crate::ProxyKind::HttpConnect => "http-connect",
            crate::ProxyKind::Socks5 => "socks5",
        },
        match session.ssh.control_master {
            crate::ControlMasterMode::Inherit => "inherit",
            crate::ControlMasterMode::Disabled => "disabled",
            crate::ControlMasterMode::Auto => "app-managed auto",
        },
        session.ssh.control_persist_seconds.is_some(),
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
        session.ssh.connect_timeout_seconds.is_some(),
        session.ssh.server_alive_interval_seconds.is_some(),
    )
}

pub fn collect(
    profile: Option<&Session>,
    explicit_config: bool,
    recent_errors: &SanitizedErrorHistory,
) -> String {
    let version = describe_version(probe(Path::new("ssh"), &["-V"], PROBE_TIMEOUT));
    let sftp = describe_usage(
        probe(Path::new("sftp"), &["-h"], PROBE_TIMEOUT),
        "usage: sftp",
    );
    let scp = describe_usage(
        probe(Path::new("scp"), &["-h"], PROBE_TIMEOUT),
        "usage: scp",
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
        "Inspirum Terminal support report - schema 2\n\
         Application: {}\n\
         Platform: {} / {}\n\
         Collection: local OpenSSH probes + allowlisted app policy + sanitized in-memory errors\n\
         SSH config source: {} (path and contents omitted)\n\
         SSH: {version}\nSFTP: {sftp}\nSCP: {scp}\nssh-keygen: {keygen}\n\
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
    match profile {
        Some(profile) => report.push_str(&policy_summary(profile)),
        None => report.push_str("Effective app launch policy: not requested\n"),
    }
    report.push_str(&recent_errors.render());
    report.push_str(
        "Excluded: hostnames, usernames, profile names, paths, keys, credentials,\n\
         passwords, passphrases, authentication responses, remote commands,\n\
         proxy/forward endpoints, arbitrary environment variables, terminal contents,\n\
         and raw connection/probe error text.\n\
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
mod tests {
    use super::*;
    use std::{fs, path::PathBuf, sync::OnceLock};

    fn helper() -> &'static Path {
        static HELPER: OnceLock<(tempfile::TempDir, PathBuf)> = OnceLock::new();
        &HELPER
            .get_or_init(|| {
                let dir = tempfile::Builder::new()
                    .prefix("support helper ")
                    .tempdir()
                    .unwrap();
                let source = dir.path().join("probe.rs");
                let program = dir
                    .path()
                    .join(format!("probe{}", std::env::consts::EXE_SUFFIX));
                fs::write(
                    &source,
                    r#"use std::{env, io::{self, Write}, thread, time::Duration};
fn main() {
    match env::args().nth(1).as_deref() {
        Some("sleep") => thread::sleep(Duration::from_secs(30)),
        Some("large") => io::stdout().write_all(&vec![b'x'; 131072]).unwrap(),
        Some("fail") => { eprintln!("PRIVATE_ERROR_CANARY"); std::process::exit(1); }
        _ => { print!("OUT"); eprint!("ERR"); }
    }
}
"#,
                )
                .unwrap();
                let status = Command::new("rustc")
                    .args(["--crate-name", "inspirum_support_probe"])
                    .arg(&source)
                    .arg("-o")
                    .arg(&program)
                    .status()
                    .unwrap();
                assert!(status.success());
                (dir, program)
            })
            .1
    }

    #[test]
    fn sanitizer_is_bounded_deterministic_and_never_copies_source_text() {
        let mut history = SanitizedErrorHistory::default();
        let canaries = [
            "Permission denied (password=PASSWORD_SECRET_CANARY)",
            "Host key verification failed HOST_SECRET_CANARY",
            "connect to 10.0.0.1 port 22: Connection refused ENDPOINT_CANARY",
            "proxy SOCKS failure PROXY_SECRET_CANARY",
            "unexpected raw text RAW_SECRET_CANARY",
        ];
        for item in canaries {
            history.record(item);
        }
        let rendered = history.render();
        assert_eq!(rendered, history.render());
        for canary in [
            "PASSWORD_SECRET_CANARY",
            "HOST_SECRET_CANARY",
            "ENDPOINT_CANARY",
            "PROXY_SECRET_CANARY",
            "RAW_SECRET_CANARY",
            "10.0.0.1",
        ] {
            assert!(!rendered.contains(canary));
        }
        assert!(rendered.contains("authentication failure"));
        assert!(rendered.contains("host-key trust or verification failure"));
        assert!(rendered.contains("connection refused"));
        assert!(rendered.contains("proxy transport failure"));
        assert!(rendered.contains("connection operation failure"));

        for index in 0..(MAX_RECENT_ERRORS + 5) {
            history.record(&format!("failure {index} SECRET"));
        }
        assert_eq!(history.len(), MAX_RECENT_ERRORS);
    }

    #[test]
    fn policy_summary_is_deterministic_and_never_contains_profile_strings() {
        let mut session = Session {
            name: "NAME_CANARY".into(),
            host: "HOST_CANARY".into(),
            user: "USER_CANARY".into(),
            strict: true,
            ..Session::default()
        };
        session.ssh.identity_file = "/private/KEY_PATH_CANARY".into();
        session.ssh.proxy_jump = "JUMP_CANARY".into();
        session.ssh.remote_command = "echo COMMAND_SECRET_CANARY".into();
        session.ssh.local_forwards = vec!["127.0.0.1:2222:FORWARD_CANARY:22".into()];
        session.ssh.password_auth = Some(false);
        session.ssh.public_key_auth = Some(true);
        let text = policy_summary(&session);
        assert_eq!(text, policy_summary(&session));
        for canary in [
            "NAME_CANARY",
            "HOST_CANARY",
            "USER_CANARY",
            "KEY_PATH_CANARY",
            "JUMP_CANARY",
            "COMMAND_SECRET_CANARY",
            "FORWARD_CANARY",
        ] {
            assert!(!text.contains(canary));
        }
        assert!(text.contains("already trusted only"));
        assert!(text.contains("public-key=enabled, password=disabled"));
        assert!(text.contains("local=1, remote=0, dynamic=0"));
    }

    #[test]
    fn version_response_and_error_output_are_never_dumped() {
        let good = Capture {
            success: true,
            stdout: "OpenSSH_9.9p1, SECRET_CANARY".into(),
            stderr: "PRIVATE_ERROR_CANARY".into(),
        };
        assert_eq!(describe_version(Ok(good)), "OpenSSH 9.9p1");
        let failure = probe(helper(), &["fail"], PROBE_TIMEOUT).unwrap();
        assert!(!failure.success);
        assert!(!describe_version(Ok(failure)).contains("PRIVATE_ERROR_CANARY"));
    }

    #[test]
    fn probes_timeout_and_reject_excess_output() {
        let start = Instant::now();
        assert!(matches!(
            probe(helper(), &["sleep"], Duration::from_millis(100)),
            Err(ProbeFailure::Timeout)
        ));
        assert!(start.elapsed() < Duration::from_secs(5));
        assert!(matches!(
            probe(helper(), &["large"], PROBE_TIMEOUT),
            Err(ProbeFailure::OutputLimit)
        ));
    }

    #[test]
    fn report_export_is_no_clobber() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("support.txt");
        export(&path, "original report").unwrap();
        assert!(export(&path, "replacement report").is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), "original report");
    }
}
