//! Native terminal adapter: egui_term owns the Alacritty parser and platform PTY.
use crate::{ControlMasterMode, ProxyKind, Session};
use anyhow::{Context, Result, ensure};
use std::{path::Path, process::Command, sync::mpsc::Sender};

const SSH_HELP: &str = "OpenSSH client unavailable. Install OpenSSH Client (Windows Settings > Optional features), macOS command-line tools, or your Linux openssh-clients package; ensure ssh is on PATH and restart Inspirum.";
const SFTP_HELP: &str = "OpenSSH sftp client unavailable. Install OpenSSH Client and ensure sftp is on PATH, then restart Inspirum.";
const SSH_KEYGEN_HELP: &str = "OpenSSH ssh-keygen unavailable. Install OpenSSH Client and ensure ssh-keygen is on PATH, then restart Inspirum.";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostKeyTarget {
    pub hostname: String,
    pub port: u16,
    pub host_key_alias: Option<String>,
    pub lookup: String,
}

pub fn check_openssh(program: &Path) -> Result<()> {
    let output = Command::new(program).arg("-V").output().context(SSH_HELP)?;
    let version = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    ensure!(
        output.status.success() && version.contains("OpenSSH_"),
        "{SSH_HELP}"
    );
    Ok(())
}
pub fn check_sftp(program: &Path) -> Result<()> {
    let output = Command::new(program)
        .arg("-h")
        .output()
        .context(SFTP_HELP)?;
    let usage = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    ensure!(usage.to_ascii_lowercase().contains("sftp"), "{SFTP_HELP}");
    Ok(())
}

fn config_path_arg(config: &Path) -> Result<String> {
    let text = config
        .to_str()
        .context("SSH config path must be valid Unicode")?;
    ensure!(!text.contains('\0'), "SSH config path contains NUL");
    Ok(text.to_owned())
}

fn safe_proxy_target(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 253
        && !value.starts_with('-')
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._-:".contains(&byte))
}

fn quote_proxy_program(path: &Path) -> Result<String> {
    let text = path
        .to_str()
        .context("proxy helper executable path must be valid Unicode")?;
    ensure!(
        !text.is_empty() && !text.chars().any(char::is_control),
        "proxy helper executable path is invalid"
    );
    #[cfg(windows)]
    {
        ensure!(
            !text.bytes().any(|byte| b"%!^&|<>\"".contains(&byte)),
            "proxy helper executable path contains characters unsafe for Windows ProxyCommand"
        );
        Ok(format!("\"{text}\""))
    }
    #[cfg(not(windows))]
    {
        Ok(format!("'{}'", text.replace('\'', "'\\''")))
    }
}

pub fn proxy_command_for_target(
    session: &Session,
    helper: &Path,
    target_host: &str,
    target_port: u16,
) -> Result<Option<String>> {
    session.ssh_args()?;
    if session.ssh.proxy_kind == ProxyKind::None {
        return Ok(None);
    }
    ensure!(
        safe_proxy_target(target_host) && target_port > 0,
        "effective proxy target contains characters unsafe for ProxyCommand"
    );
    let proxy_port = session
        .ssh
        .proxy_port
        .context("structured proxy port is missing")?;
    ensure!(proxy_port > 0, "structured proxy port must be non-zero");
    let mode = match session.ssh.proxy_kind {
        ProxyKind::HttpConnect => "http-connect",
        ProxyKind::Socks5 => "socks5",
        ProxyKind::None => unreachable!(),
    };
    let program = quote_proxy_program(helper)?;
    Ok(Some(format!(
        "{program} --proxy-helper --mode {mode} --proxy-host {} --proxy-port {proxy_port} --target-host {target_host} --target-port {target_port}",
        session.ssh.proxy_host
    )))
}

fn structured_proxy_option(
    session: &Session,
    config: Option<&Path>,
    helper: &Path,
) -> Result<Option<String>> {
    if session.ssh.proxy_kind == ProxyKind::None {
        return Ok(None);
    }
    let target = resolve_host_key_target(session, config)?;
    proxy_command_for_target(session, helper, &target.hostname, target.port)
}

pub fn launch_args_with_proxy_helper(
    session: &Session,
    config: Option<&Path>,
    helper: &Path,
) -> Result<Vec<String>> {
    let mut args = session.ssh_args()?;
    let index = args
        .iter()
        .position(|arg| arg == "--")
        .context("internal SSH argv is missing option terminator")?;
    let mut extra = Vec::new();
    if let Some(config) = config {
        extra.extend(["-F".into(), config_path_arg(config)?]);
    }
    if let Some(command) = structured_proxy_option(session, config, helper)? {
        extra.extend(["-o".into(), format!("ProxyCommand={command}")]);
    }
    args.splice(index..index, extra);
    Ok(args)
}

pub fn control_master_args(session: &Session, config: Option<&Path>, operation: &str) -> Result<Vec<String>> {
    ensure!(operation == "check" || operation == "exit", "unsupported ControlMaster operation");
    ensure!(session.ssh.control_master == ControlMasterMode::Auto, "app-managed ControlMaster is not enabled");
    session.ssh_args()?;
    let mut args = Vec::new();
    if let Some(config) = config {
        args.extend(["-F".into(), config_path_arg(config)?]);
    }
    args.extend(["-S".into(), session.ssh.control_path.clone()]);
    args.extend(["-O".into(), operation.into()]);
    if !session.user.is_empty() { args.extend(["-l".into(), session.user.clone()]); }
    if let Some(port) = session.port { args.extend(["-p".into(), port.to_string()]); }
    args.extend(["--".into(), session.host.clone()]);
    Ok(args)
}

pub fn control_master_operation(session: &Session, config: Option<&Path>, operation: &str) -> Result<String> {
    let output = Command::new("ssh").args(control_master_args(session, config, operation)?).output().context(SSH_HELP)?;
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    ensure!(output.status.success(), "ControlMaster {operation} failed: {}", if stderr.is_empty() { "master unavailable or stale ControlPath" } else { &stderr });
    Ok(if stdout.is_empty() { stderr } else { stdout })
}

pub fn launch_args(session: &Session, config: Option<&Path>) -> Result<Vec<String>> {
    if session.ssh.proxy_kind == ProxyKind::None {
        return launch_args_with_proxy_helper(session, config, Path::new(""));
    }
    let helper = std::env::current_exe().context("locate Inspirum proxy helper executable")?;
    launch_args_with_proxy_helper(session, config, &helper)
}

fn host_key_query_args(session: &Session, config: Option<&Path>) -> Result<Vec<String>> {
    // Validate the complete profile before deriving the effective OpenSSH destination.
    session.ssh_args()?;
    let mut args = vec!["-G".into()];
    if let Some(config) = config {
        args.extend(["-F".into(), config_path_arg(config)?]);
    }
    if !session.user.is_empty() {
        args.extend(["-l".into(), session.user.clone()]);
    }
    if let Some(port) = session.port {
        args.extend(["-p".into(), port.to_string()]);
    }
    args.extend(["--".into(), session.host.clone()]);
    Ok(args)
}

fn known_hosts_lookup(hostname: &str, port: u16, host_key_alias: Option<&str>) -> String {
    if let Some(alias) = host_key_alias {
        return alias.to_owned();
    }
    if port == 22 {
        hostname.to_owned()
    } else {
        format!("[{hostname}]:{port}")
    }
}

pub fn parse_host_key_target(config: &str) -> Result<HostKeyTarget> {
    let mut hostname = None;
    let mut port = None;
    let mut host_key_alias = None;

    for line in config.lines() {
        let Some((key, value)) = line.split_once(char::is_whitespace) else {
            continue;
        };
        let value = value.trim();
        match key.to_ascii_lowercase().as_str() {
            "hostname" if !value.is_empty() => hostname = Some(value.to_owned()),
            "port" => {
                let parsed = value
                    .parse::<u16>()
                    .context("invalid port in ssh -G output")?;
                ensure!(parsed > 0, "invalid zero port in ssh -G output");
                port = Some(parsed);
            }
            "hostkeyalias" if !value.is_empty() && !value.eq_ignore_ascii_case("none") => {
                host_key_alias = Some(value.to_owned());
            }
            _ => {}
        }
    }

    let hostname = hostname.context("ssh -G output did not contain hostname")?;
    let port = port.unwrap_or(22);
    let lookup = known_hosts_lookup(&hostname, port, host_key_alias.as_deref());
    Ok(HostKeyTarget {
        hostname,
        port,
        host_key_alias,
        lookup,
    })
}

pub fn resolve_host_key_target(session: &Session, config: Option<&Path>) -> Result<HostKeyTarget> {
    let args = host_key_query_args(session, config)?;
    check_openssh(Path::new("ssh"))?;
    let output = Command::new("ssh")
        .args(args)
        .output()
        .context("query effective OpenSSH destination with ssh -G")?;
    ensure!(
        output.status.success(),
        "ssh -G failed: {}",
        String::from_utf8_lossy(&output.stderr).trim()
    );
    let text = String::from_utf8(output.stdout).context("ssh -G output was not UTF-8")?;
    parse_host_key_target(&text)
}

fn ssh_keygen_args(action: &str, lookup: &str, known_hosts: Option<&Path>) -> Result<Vec<String>> {
    ensure!(
        action == "-F" || action == "-R",
        "unsupported ssh-keygen known-hosts action"
    );
    ensure!(
        !lookup.is_empty() && !lookup.chars().any(char::is_control),
        "known-hosts lookup target is invalid"
    );
    let mut args = vec![action.to_owned(), lookup.to_owned()];
    if let Some(path) = known_hosts {
        ensure!(
            path.is_file(),
            "known_hosts override does not exist or is not a regular file: {}",
            path.display()
        );
        let value = path
            .to_str()
            .context("known_hosts path must be valid Unicode")?;
        ensure!(!value.contains('\0'), "known_hosts path contains NUL");
        args.extend(["-f".into(), value.to_owned()]);
    }
    Ok(args)
}

pub fn inspect_known_host(target: &HostKeyTarget, known_hosts: Option<&Path>) -> Result<String> {
    let args = ssh_keygen_args("-F", &target.lookup, known_hosts)?;
    let output = Command::new("ssh-keygen")
        .args(args)
        .output()
        .context(SSH_KEYGEN_HELP)?;
    let matches = match output.status.code() {
        Some(0) => String::from_utf8_lossy(&output.stdout).trim().to_owned(),
        Some(1) => return Ok(String::new()),
        _ => anyhow::bail!(
            "ssh-keygen -F failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ),
    };

    let mut matched_file =
        tempfile::NamedTempFile::new().context("create temporary host-key fingerprint input")?;
    std::io::Write::write_all(&mut matched_file, matches.as_bytes())?;
    matched_file.as_file().sync_all()?;
    let matched_path = matched_file.into_temp_path();
    let fingerprint_output = Command::new("ssh-keygen")
        .arg("-l")
        .arg("-f")
        .arg(&matched_path)
        .output()
        .context(SSH_KEYGEN_HELP)?;
    ensure!(
        fingerprint_output.status.success(),
        "ssh-keygen fingerprint failed: {}",
        String::from_utf8_lossy(&fingerprint_output.stderr).trim()
    );
    let fingerprints = String::from_utf8_lossy(&fingerprint_output.stdout)
        .trim()
        .to_owned();
    Ok(format!("{matches}\nFingerprints:\n{fingerprints}"))
}

pub fn remove_known_host(target: &HostKeyTarget, known_hosts: Option<&Path>) -> Result<String> {
    let args = ssh_keygen_args("-R", &target.lookup, known_hosts)?;
    let output = Command::new("ssh-keygen")
        .args(args)
        .output()
        .context(SSH_KEYGEN_HELP)?;
    ensure!(
        output.status.success(),
        "ssh-keygen -R failed: {}",
        String::from_utf8_lossy(&output.stderr).trim()
    );
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    Ok(if stdout.is_empty() { stderr } else { stdout })
}

pub fn sftp_launch_args_with_proxy_helper(
    session: &Session,
    config: Option<&Path>,
    helper: &Path,
) -> Result<Vec<String>> {
    // Reuse the SSH profile validator, then map only options that apply to sftp.
    session.ssh_args()?;
    let mut args = vec![
        "-o".into(),
        format!(
            "StrictHostKeyChecking={}",
            if session.strict { "yes" } else { "ask" }
        ),
    ];
    if let Some(config) = config {
        args.extend(["-F".into(), config_path_arg(config)?]);
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
            args.extend(["-o".into(), format!("ControlPath={}", session.ssh.control_path)]);
            if let Some(seconds) = session.ssh.control_persist_seconds {
                args.extend(["-o".into(), format!("ControlPersist={seconds}")]);
            }
        }
    }
    if let Some(command) = structured_proxy_option(session, config, helper)? {
        args.extend(["-o".into(), format!("ProxyCommand={command}")]);
    }
    // Authentication policy is not agent forwarding. SFTP must honor method restrictions
    // and delegation decisions even though terminal-only features are intentionally absent.
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
    // sftp parses host:path, unlike ssh. Brackets keep an IPv6 literal (and optional
    // zone identifier) a host instead of accidentally requesting an automatic download.
    args.push(if session.host.contains(':') {
        format!("[{}]", session.host)
    } else {
        session.host.clone()
    });
    Ok(args)
}

pub fn sftp_launch_args(session: &Session, config: Option<&Path>) -> Result<Vec<String>> {
    if session.ssh.proxy_kind == ProxyKind::None {
        return sftp_launch_args_with_proxy_helper(session, config, Path::new(""));
    }
    let helper = std::env::current_exe().context("locate Inspirum proxy helper executable")?;
    sftp_launch_args_with_proxy_helper(session, config, &helper)
}

pub fn connect(
    id: u64,
    context: eframe::egui::Context,
    sender: Sender<(u64, egui_term::PtyEvent)>,
    session: &Session,
    config: Option<&Path>,
) -> Result<egui_term::TerminalBackend> {
    let args = launch_args(session, config)?;
    check_openssh(Path::new("ssh"))?;
    egui_term::TerminalBackend::new(
        id,
        context,
        sender,
        egui_term::BackendSettings {
            shell: "ssh".into(),
            args,
            working_directory: None,
        },
    )
    .context("create native PTY and start OpenSSH")
}

pub fn connect_sftp(
    id: u64,
    context: eframe::egui::Context,
    sender: Sender<(u64, egui_term::PtyEvent)>,
    session: &Session,
    config: Option<&Path>,
) -> Result<egui_term::TerminalBackend> {
    let args = sftp_launch_args(session, config)?;
    check_sftp(Path::new("sftp"))?;
    egui_term::TerminalBackend::new(
        id,
        context,
        sender,
        egui_term::BackendSettings {
            shell: "sftp".into(),
            args,
            working_directory: None,
        },
    )
    .context("create native PTY and start OpenSSH sftp")
}
