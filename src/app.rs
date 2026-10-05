//! Small native connection/profile interface; terminal mechanics stay upstream.
use crate::{
    Session, delete_session, duplicate_session_draft, load_sessions, save_session_edit,
    save_sessions, session_matches_query, terminal,
};
use eframe::egui;
use egui_term::{PtyEvent, TerminalBackend, TerminalView};
use std::{
    path::PathBuf,
    sync::mpsc::{self, Receiver, Sender},
};

#[derive(Clone, Copy)]
enum TabKind {
    Ssh,
    Sftp,
}

impl TabKind {
    fn connect(
        self,
        id: u64,
        context: egui::Context,
        sender: Sender<(u64, PtyEvent)>,
        session: &Session,
        config: Option<&std::path::Path>,
    ) -> anyhow::Result<TerminalBackend> {
        match self {
            Self::Ssh => terminal::connect(id, context, sender, session, config),
            Self::Sftp => terminal::connect_sftp(id, context, sender, session, config),
        }
    }
}

struct Tab {
    id: u64,
    name: String,
    kind: TabKind,
    session: Session,
    terminal: TerminalBackend,
    exited: bool,
}

fn terminal_accepts_keyboard(owner: Option<u64>, id: u64, exited: bool) -> bool {
    owner == Some(id) && !exited
}

fn parse_optional_u16(value: &str, label: &str) -> anyhow::Result<Option<u16>> {
    let value = value.trim();
    if value.is_empty() {
        return Ok(None);
    }
    let parsed = value
        .parse::<u16>()
        .map_err(|_| anyhow::anyhow!("{label} must be a whole number from 1 to 65535"))?;
    anyhow::ensure!(parsed > 0, "{label} must be greater than zero");
    Ok(Some(parsed))
}

fn parse_forward_lines(value: &str) -> Vec<String> {
    value
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

fn ssh_policy_control(ui: &mut egui::Ui, label: &str, value: &mut Option<bool>) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.label(label);
        changed |= ui.selectable_value(value, None, "Inherit").changed();
        changed |= ui.selectable_value(value, Some(true), "Enable").changed();
        changed |= ui.selectable_value(value, Some(false), "Disable").changed();
    });
    changed
}

pub struct App {
    path: PathBuf,
    config: Option<PathBuf>,
    profiles: Vec<Session>,
    profile_query: String,
    selected_profile: Option<String>,
    delete_confirm: Option<String>,
    draft: Session,
    port: String,
    connect_timeout: String,
    keepalive: String,
    local_forwards: String,
    remote_forwards: String,
    dynamic_forwards: String,
    error: String,
    writable: bool,
    tabs: Vec<Tab>,
    active: Option<u64>,
    terminal_focus: Option<u64>,
    next_id: u64,
    tx: Sender<(u64, PtyEvent)>,
    rx: Receiver<(u64, PtyEvent)>,
}

impl App {
    pub fn new(path: PathBuf, config: Option<PathBuf>) -> Self {
        let (profiles, error, writable) = match load_sessions(&path) {
            Ok(profiles) => (profiles, String::new(), true),
            Err(error) => (
                Vec::new(),
                format!("{error:#}. Saving disabled: repair the profile file and restart."),
                false,
            ),
        };
        let (tx, rx) = mpsc::channel();
        Self {
            path,
            config,
            profiles,
            profile_query: String::new(),
            selected_profile: None,
            delete_confirm: None,
            draft: Session::default(),
            port: String::new(),
            connect_timeout: String::new(),
            keepalive: String::new(),
            local_forwards: String::new(),
            remote_forwards: String::new(),
            dynamic_forwards: String::new(),
            error,
            writable,
            tabs: Vec::new(),
            active: None,
            terminal_focus: None,
            next_id: 1,
            tx,
            rx,
        }
    }

    pub fn storage_writable(&self) -> bool {
        self.writable
    }

    fn load_draft(&mut self, session: Session) {
        self.port = session.port.map(|p| p.to_string()).unwrap_or_default();
        self.connect_timeout = session
            .ssh
            .connect_timeout_seconds
            .map(|value| value.to_string())
            .unwrap_or_default();
        self.keepalive = session
            .ssh
            .server_alive_interval_seconds
            .map(|value| value.to_string())
            .unwrap_or_default();
        self.local_forwards = session.ssh.local_forwards.join("\n");
        self.remote_forwards = session.ssh.remote_forwards.join("\n");
        self.dynamic_forwards = session.ssh.dynamic_forwards.join("\n");
        self.draft = session;
    }

    fn validated_draft(&self) -> anyhow::Result<Session> {
        let mut session = self.draft.clone();
        session.port = parse_optional_u16(&self.port, "port")?;
        session.ssh.connect_timeout_seconds =
            parse_optional_u16(&self.connect_timeout, "connection timeout")?;
        session.ssh.server_alive_interval_seconds =
            parse_optional_u16(&self.keepalive, "keepalive interval")?;
        session.ssh.local_forwards = parse_forward_lines(&self.local_forwards);
        session.ssh.remote_forwards = parse_forward_lines(&self.remote_forwards);
        session.ssh.dynamic_forwards = parse_forward_lines(&self.dynamic_forwards);
        session.ssh_args()?;
        Ok(session)
    }

    pub fn ui(&mut self, ctx: &egui::Context) {
        // Remote output may set titles/clipboard requests. Do NOT forward those to host APIs.
        for _ in 0..256 {
            let Ok((id, event)) = self.rx.try_recv() else {
                break;
            };
            if matches!(event, PtyEvent::Exit)
                && let Some(tab) = self.tabs.iter_mut().find(|tab| tab.id == id)
            {
                tab.exited = true;
            }
        }

        egui::SidePanel::left("connections")
            .resizable(true)
            .default_width(330.0)
            .show(ctx, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    ui.heading("Inspirum Terminal");
                    ui.label("SSH-first • system OpenSSH");
                    ui.separator();
                    ui.label("Saved sessions");
                    ui.horizontal(|ui| {
                        if ui
                            .add(
                                egui::TextEdit::singleline(&mut self.profile_query)
                                    .hint_text("Search name, host or user"),
                            )
                            .has_focus()
                        {
                            self.terminal_focus = None;
                        }
                        if ui.small_button("New").clicked() {
                            self.terminal_focus = None;
                            self.selected_profile = None;
                            self.delete_confirm = None;
                            self.load_draft(Session::default());
                        }
                    });

                    let mut selected = None;
                    egui::ScrollArea::vertical()
                        .max_height(150.0)
                        .show(ui, |ui| {
                            for profile in self
                                .profiles
                                .iter()
                                .filter(|profile| session_matches_query(profile, &self.profile_query))
                            {
                                let is_selected =
                                    self.selected_profile.as_deref() == Some(profile.name.as_str());
                                if ui.selectable_label(is_selected, &profile.name).clicked() {
                                    selected = Some(profile.clone());
                                }
                            }
                        });
                    if let Some(profile) = selected {
                        self.terminal_focus = None;
                        self.selected_profile = Some(profile.name.clone());
                        self.delete_confirm = None;
                        self.load_draft(profile);
                    }

                    if let Some(selected_name) = self.selected_profile.clone() {
                        ui.horizontal(|ui| {
                            if ui.small_button("Duplicate").clicked()
                                && let Some(source) = self
                                    .profiles
                                    .iter()
                                    .find(|profile| profile.name == selected_name)
                                    .cloned()
                            {
                                let draft = duplicate_session_draft(&self.profiles, &source);
                                self.terminal_focus = None;
                                self.selected_profile = None;
                                self.delete_confirm = None;
                                self.load_draft(draft);
                            }
                            if ui
                                .small_button("Delete")
                                .on_hover_text("Delete the selected saved profile")
                                .clicked()
                            {
                                self.terminal_focus = None;
                                self.delete_confirm = Some(selected_name.clone());
                            }
                        });
                    }

                    if let Some(name) = self.delete_confirm.clone() {
                        ui.group(|ui| {
                            ui.label(format!("Delete saved profile {name:?}?"));
                            ui.small("This removes only the saved profile. Open SSH/SFTP tabs are not disconnected.");
                            ui.horizontal(|ui| {
                                if ui.button("Confirm delete").clicked() {
                                    let result = (|| -> anyhow::Result<()> {
                                        let next = delete_session(&self.profiles, &name)?;
                                        save_sessions(&self.path, &next)?;
                                        self.profiles = next;
                                        self.delete_confirm = None;
                                        if self.selected_profile.as_deref() == Some(name.as_str()) {
                                            self.selected_profile = None;
                                            self.load_draft(Session::default());
                                        }
                                        Ok(())
                                    })();
                                    self.error = result
                                        .err()
                                        .map(|error| format!("{error:#}"))
                                        .unwrap_or_default();
                                }
                                if ui.button("Cancel").clicked() {
                                    self.delete_confirm = None;
                                }
                            });
                        });
                    }

                    ui.separator();
                    ui.label("Name");
                    if ui.text_edit_singleline(&mut self.draft.name).has_focus() {
                        self.terminal_focus = None;
                    }
                    ui.label("Host / SSH config alias");
                    if ui.text_edit_singleline(&mut self.draft.host).has_focus() {
                        self.terminal_focus = None;
                    }
                    ui.label("Username (blank: SSH config/default)");
                    if ui.text_edit_singleline(&mut self.draft.user).has_focus() {
                        self.terminal_focus = None;
                    }
                    ui.label("Port (blank: SSH config/default)");
                    if ui.text_edit_singleline(&mut self.port).has_focus() {
                        self.terminal_focus = None;
                    }
                    if ui
                        .checkbox(
                            &mut self.draft.strict,
                            "Require already trusted host key",
                        )
                        .clicked()
                    {
                        self.terminal_focus = None;
                    }
                    ui.small(
                        "Unchecked: OpenSSH asks before trusting a new host. Changed host keys are rejected.",
                    );

                    egui::CollapsingHeader::new("Advanced SSH")
                        .default_open(false)
                        .show(ui, |ui| {
                            ui.label("Identity file path");
                            if ui
                                .text_edit_singleline(&mut self.draft.ssh.identity_file)
                                .has_focus()
                            {
                                self.terminal_focus = None;
                            }
                            ui.small(
                                "Only the path is stored. Passphrases and private-key contents remain with OpenSSH.",
                            );

                            ui.label("ProxyJump");
                            if ui
                                .text_edit_singleline(&mut self.draft.ssh.proxy_jump)
                                .has_focus()
                            {
                                self.terminal_focus = None;
                            }
                            ui.small("Example: bastion or user@bastion:2222,second-hop");

                            ui.separator();
                            ui.strong("Authentication");
                            ui.small(
                                "Inherit preserves OpenSSH config/defaults. These controls choose which authentication methods OpenSSH may attempt; credentials still stay inside OpenSSH.",
                            );
                            if ssh_policy_control(
                                ui,
                                "Public key",
                                &mut self.draft.ssh.public_key_auth,
                            ) {
                                self.terminal_focus = None;
                            }
                            if ssh_policy_control(
                                ui,
                                "Password",
                                &mut self.draft.ssh.password_auth,
                            ) {
                                self.terminal_focus = None;
                            }
                            if ssh_policy_control(
                                ui,
                                "Keyboard-interactive / MFA",
                                &mut self.draft.ssh.keyboard_interactive_auth,
                            ) {
                                self.terminal_focus = None;
                            }
                            if ssh_policy_control(
                                ui,
                                "GSSAPI",
                                &mut self.draft.ssh.gssapi_auth,
                            ) {
                                self.terminal_focus = None;
                            }
                            if ssh_policy_control(
                                ui,
                                "Delegate GSSAPI credentials",
                                &mut self.draft.ssh.gssapi_delegate_credentials,
                            ) {
                                self.terminal_focus = None;
                            }
                            ui.small(
                                "GSSAPI support is platform/OpenSSH-build dependent. Credential delegation can expose delegated credentials to the remote host; enable only when required.",
                            );
                            if ssh_policy_control(
                                ui,
                                "Use configured identities only",
                                &mut self.draft.ssh.identities_only,
                            ) {
                                self.terminal_focus = None;
                            }
                            ui.small(
                                "Enabling IdentitiesOnly limits public-key authentication to explicitly configured identities and certificate files instead of every key offered by an agent.",
                            );

                            ui.separator();
                            if ssh_policy_control(
                                ui,
                                "SSH agent forwarding",
                                &mut self.draft.ssh.agent_forwarding,
                            ) {
                                self.terminal_focus = None;
                            }
                            ui.small(
                                "Inherit follows OpenSSH config. Enabling lets the remote host use your local agent; enable only for trusted hosts.",
                            );

                            if ssh_policy_control(
                                ui,
                                "X11 forwarding",
                                &mut self.draft.ssh.x11_forwarding,
                            ) {
                                self.terminal_focus = None;
                            }
                            ui.small(
                                "Inherit follows OpenSSH config. Enabling lets remote applications connect to your local X server when available; enable only when needed and for trusted hosts.",
                            );
                            if ssh_policy_control(
                                ui,
                                "Compression",
                                &mut self.draft.ssh.compression,
                            ) {
                                self.terminal_focus = None;
                            }
                            ui.small("Inherit follows the effective OpenSSH configuration.");

                            ui.label("Connection timeout seconds");
                            if ui.text_edit_singleline(&mut self.connect_timeout).has_focus() {
                                self.terminal_focus = None;
                            }
                            ui.label("Server keepalive interval seconds");
                            if ui.text_edit_singleline(&mut self.keepalive).has_focus() {
                                self.terminal_focus = None;
                            }

                            ui.label("Command after authentication (remote)");
                            if ui
                                .add(
                                    egui::TextEdit::multiline(&mut self.draft.ssh.remote_command)
                                        .desired_rows(2)
                                        .hint_text("tmux attach || tmux new"),
                                )
                                .has_focus()
                            {
                                self.terminal_focus = None;
                            }
                            ui.small(
                                "Sent to the remote account after SSH authentication. It is not executed by a local shell; the remote account's shell interprets it.",
                            );

                            ui.separator();
                            ui.strong("Port forwarding");
                            ui.small(
                                "One OpenSSH forwarding specification per line. These are added to any forwards from OpenSSH config. Profile forwarding is tied to this SSH session and fails the connection if setup fails.",
                            );

                            ui.label("Local forwards (-L)");
                            if ui
                                .add(
                                    egui::TextEdit::multiline(&mut self.local_forwards)
                                        .desired_rows(2)
                                        .hint_text("127.0.0.1:8080:internal.example:80"),
                                )
                                .has_focus()
                            {
                                self.terminal_focus = None;
                            }

                            ui.label("Remote forwards (-R)");
                            if ui
                                .add(
                                    egui::TextEdit::multiline(&mut self.remote_forwards)
                                        .desired_rows(2)
                                        .hint_text("127.0.0.1:9000:127.0.0.1:3000"),
                                )
                                .has_focus()
                            {
                                self.terminal_focus = None;
                            }

                            ui.label("Dynamic forwards (-D)");
                            if ui
                                .add(
                                    egui::TextEdit::multiline(&mut self.dynamic_forwards)
                                        .desired_rows(2)
                                        .hint_text("127.0.0.1:1080"),
                                )
                                .has_focus()
                            {
                                self.terminal_focus = None;
                            }
                        });

                    ui.horizontal(|ui| {
                        if ui
                            .add_enabled(self.writable, egui::Button::new("Save profile"))
                            .clicked()
                        {
                            self.terminal_focus = None;
                            let selected_name = self.selected_profile.clone();
                            let result = self.validated_draft().and_then(|session| {
                                let saved_name = session.name.clone();
                                let next = save_session_edit(
                                    &self.profiles,
                                    selected_name.as_deref(),
                                    session,
                                )?;
                                save_sessions(&self.path, &next)?;
                                self.profiles = next;
                                self.selected_profile = Some(saved_name);
                                self.delete_confirm = None;
                                Ok(())
                            });
                            self.error =
                                result.err().map(|e| format!("{e:#}")).unwrap_or_default();
                        }
                        if ui
                            .add_enabled(self.tabs.len() < 16, egui::Button::new("Connect"))
                            .clicked()
                        {
                            let result = self.validated_draft().and_then(|session| {
                                let terminal = terminal::connect(
                                    self.next_id,
                                    ctx.clone(),
                                    self.tx.clone(),
                                    &session,
                                    self.config.as_deref(),
                                )?;
                                self.tabs.push(Tab {
                                    id: self.next_id,
                                    name: session.name.clone(),
                                    kind: TabKind::Ssh,
                                    session,
                                    terminal,
                                    exited: false,
                                });
                                self.active = Some(self.next_id);
                                self.terminal_focus = Some(self.next_id);
                                self.next_id += 1;
                                Ok(())
                            });
                            self.error =
                                result.err().map(|e| format!("{e:#}")).unwrap_or_default();
                        }
                        if ui
                            .add_enabled(self.tabs.len() < 16, egui::Button::new("SFTP"))
                            .on_hover_text("Open an interactive OpenSSH sftp session")
                            .clicked()
                        {
                            let result = self.validated_draft().and_then(|session| {
                                let terminal = terminal::connect_sftp(
                                    self.next_id,
                                    ctx.clone(),
                                    self.tx.clone(),
                                    &session,
                                    self.config.as_deref(),
                                )?;
                                self.tabs.push(Tab {
                                    id: self.next_id,
                                    name: format!("{} · SFTP", session.name),
                                    kind: TabKind::Sftp,
                                    session,
                                    terminal,
                                    exited: false,
                                });
                                self.active = Some(self.next_id);
                                self.terminal_focus = Some(self.next_id);
                                self.next_id += 1;
                                Ok(())
                            });
                            self.error =
                                result.err().map(|e| format!("{e:#}")).unwrap_or_default();
                        }
                    });
                    ui.small(
                        "SFTP reuses host trust, OpenSSH config, identity, ProxyJump, timeout, keepalive and compression. Terminal-only remote commands, X11/agent forwarding and port forwards are not applied to SFTP.",
                    );

                    if !self.error.is_empty() {
                        ui.colored_label(egui::Color32::LIGHT_RED, &self.error);
                    }
                    ui.separator();
                    ui.small(
                        "Authentication happens only inside OpenSSH's PTY. Passwords, passphrases and private-key contents are not saved by this app.",
                    );
                    ui.small(format!("Profiles: {}", self.path.display()));
                    if let Some(config) = &self.config {
                        ui.small(format!("SSH config: {}", config.display()));
                    }
                });
            });

        egui::TopBottomPanel::top("tabs").show(ctx, |ui| {
            let mut close = None;
            ui.horizontal_wrapped(|ui| {
                for tab in &self.tabs {
                    let label =
                        format!("{}{}", tab.name, if tab.exited { " (exited)" } else { "" });
                    if ui
                        .selectable_label(self.active == Some(tab.id), label)
                        .clicked()
                    {
                        self.active = Some(tab.id);
                        self.terminal_focus = Some(tab.id);
                    }
                    if ui
                        .small_button("×")
                        .on_hover_text("Disconnect and close terminal")
                        .clicked()
                    {
                        close = Some(tab.id);
                    }
                }
            });
            if let Some(id) = close {
                self.tabs.retain(|tab| tab.id != id);
                if self.active == Some(id) {
                    self.active = self.tabs.last().map(|tab| tab.id);
                }
                if self.terminal_focus == Some(id) {
                    self.terminal_focus = self.active;
                }
            }
        });

        let mut reconnect = None;
        egui::CentralPanel::default().show(ctx, |ui| {
            let terminal_focus = self.terminal_focus;
            if let Some(tab) = self.tabs.iter_mut().find(|tab| Some(tab.id) == self.active) {
                if tab.exited {
                    ui.horizontal(|ui| {
                        ui.strong("Session exited.");
                        if ui.button("Reconnect").clicked() {
                            reconnect = Some((tab.id, tab.kind, tab.session.clone()));
                        }
                    });
                    ui.separator();
                }
                let view = TerminalView::new(ui, &mut tab.terminal)
                    .set_focus(terminal_accepts_keyboard(terminal_focus, tab.id, tab.exited));
                let response = ui.add(view);
                if response.clicked() && !tab.exited {
                    self.terminal_focus = Some(tab.id);
                }
            } else {
                ui.heading("Connect to an SSH server");
                ui.label("Enter a host or existing ~/.ssh/config alias, then Connect.");
                ui.label(
                    "Advanced SSH profiles support identity files, ProxyJump, forwarding, keepalive, X11 and agent forwarding.",
                );
                ui.label("Click the terminal to type. Close a tab to disconnect.");
                ui.label(
                    "Verify host key fingerprints through an independent trusted channel before accepting.",
                );
                ui.label("No sessions are automatically connected on startup.");
            }
        });

        if let Some((old_id, kind, session)) = reconnect {
            let new_id = self.next_id;
            match kind.connect(
                new_id,
                ctx.clone(),
                self.tx.clone(),
                &session,
                self.config.as_deref(),
            ) {
                Ok(terminal) => {
                    if let Some(tab) = self.tabs.iter_mut().find(|tab| tab.id == old_id) {
                        tab.id = new_id;
                        tab.kind = kind;
                        tab.session = session;
                        tab.terminal = terminal;
                        tab.exited = false;
                        self.active = Some(new_id);
                        self.terminal_focus = Some(new_id);
                        self.next_id += 1;
                        self.error.clear();
                    }
                }
                Err(error) => {
                    self.error = format!("{error:#}");
                }
            }
        }
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.ui(ctx);
    }
}
