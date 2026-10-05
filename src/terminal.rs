//! Native terminal adapter: egui_term owns the Alacritty parser and platform PTY.
use anyhow::{Context, Result, ensure};
use std::{path::Path, process::Command, sync::mpsc::Sender};
use crate::Session;

const SSH_HELP: &str = "OpenSSH client unavailable. Install OpenSSH Client (Windows Settings > Optional features), macOS command-line tools, or your Linux openssh-clients package; ensure ssh is on PATH and restart Inspirum.";

pub fn check_openssh(program: &Path) -> Result<()> {
    let output = Command::new(program).arg("-V").output().context(SSH_HELP)?;
    let version = format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    ensure!(output.status.success() && version.contains("OpenSSH_"), "{SSH_HELP}");
    Ok(())
}
pub fn launch_args(session: &Session, config: Option<&Path>) -> Result<Vec<String>> {
    let mut args = session.ssh_args()?;
    if let Some(config) = config {
        let text = config.to_str().context("SSH config path must be valid Unicode")?;
        ensure!(!text.contains('\0'), "SSH config path contains NUL");
        let index = args.len() - 2;
        args.splice(index..index, ["-F".into(), text.to_owned()]);
    }
    Ok(args)
}
pub fn connect(id: u64, context: eframe::egui::Context, sender: Sender<(u64, egui_term::PtyEvent)>, session: &Session, config: Option<&Path>) -> Result<egui_term::TerminalBackend> {
    let args = launch_args(session, config)?;
    check_openssh(Path::new("ssh"))?;
    egui_term::TerminalBackend::new(id, context, sender, egui_term::BackendSettings {
        shell: "ssh".into(), args, working_directory: None,
    }).context("create native PTY and start OpenSSH")
}
