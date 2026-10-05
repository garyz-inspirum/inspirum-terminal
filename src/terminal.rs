//! Native terminal adapter: egui_term owns the Alacritty parser and platform PTY.
use crate::Session;
use anyhow::{Context, Result, ensure};
use std::{path::Path, process::Command, sync::mpsc::Sender};

const SSH_HELP: &str = "OpenSSH client unavailable. Install OpenSSH Client (Windows Settings > Optional features), macOS command-line tools, or your Linux openssh-clients package; ensure ssh is on PATH and restart Inspirum.";
const SFTP_HELP: &str = "OpenSSH sftp client unavailable. Install OpenSSH Client and ensure sftp is on PATH, then restart Inspirum.";

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
    let output = Command::new(program).arg("-h").output().context(SFTP_HELP)?;
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

pub fn launch_args(session: &Session, config: Option<&Path>) -> Result<Vec<String>> {
    let mut args = session.ssh_args()?;
    if let Some(config) = config {
        let text = config_path_arg(config)?;
        let index = args
            .iter()
            .position(|arg| arg == "--")
            .context("internal SSH argv is missing option terminator")?;
        args.splice(index..index, ["-F".into(), text]);
    }
    Ok(args)
}

pub fn sftp_launch_args(session: &Session, config: Option<&Path>) -> Result<Vec<String>> {
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
    args.push(session.host.clone());
    Ok(args)
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
