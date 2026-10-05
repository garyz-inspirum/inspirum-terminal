//! Small native connection/profile interface; terminal mechanics stay upstream.
use crate::{
    ControlMasterMode, ProxyKind, Session, SessionImportMode, delete_session,
    duplicate_session_draft, export_sessions, import_sessions, load_sessions, save_session_edit,
    save_sessions,
    scp_panel::ScpPanel,
    session_matches_query,
    sftp_browser::SftpBrowser,
    support::{self, SanitizedErrorHistory},
    terminal,
    terminal_ux::{self, PasteDecision, PastePolicy},
    tmux::{self, TmuxSession},
};
use eframe::egui;
use egui_term::{BackendCommand, PtyEvent, TerminalBackend, TerminalView};
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

fn terminal_screen_text(terminal: &mut TerminalBackend) -> String {
    let content = terminal.sync();
    let mut result = String::new();
    let mut current_line = None;

    for indexed in content.grid.display_iter() {
        if current_line != Some(indexed.point.line) {
            if current_line.is_some() {
                while result.ends_with(' ') {
                    result.pop();
                }
                result.push('\n');
            }
            current_line = Some(indexed.point.line);
        }
        result.push(indexed.c);
    }

    result.trim_end_matches([' ', '\n']).to_owned()
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

fn forward_editor(ui: &mut egui::Ui, label: &str, value: &mut String, default_spec: &str) -> bool {
    ui.label(label);
    let mut rows = parse_forward_lines(value);
    let mut remove = None;
    let mut focused = false;
    for (index, row) in rows.iter_mut().enumerate() {
        ui.horizontal(|ui| {
            focused |= ui.text_edit_singleline(row).has_focus();
            if ui.small_button("Remove").clicked() {
                remove = Some(index);
            }
        });
    }
    if let Some(index) = remove {
        rows.remove(index);
    }
    if ui.small_button(format!("Add {label}")).clicked() {
        rows.push(default_spec.to_owned());
    }
    *value = rows.join("\n");
    focused
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
    profile_transfer_path: String,
    replace_import_confirm: Option<PathBuf>,
    profile_transfer_notice: String,
    draft: Session,
    port: String,
    proxy_port: String,
    control_persist: String,
    control_master_notice: String,
    connect_timeout: String,
    keepalive: String,
    local_forwards: String,
    remote_forwards: String,
    dynamic_forwards: String,
    forward_risk_ack: bool,
    tunnel_process: Option<terminal::TunnelProcess>,
    tunnel_notice: String,
    sftp_browser: Option<SftpBrowser>,
    scp_panel: Option<ScpPanel>,
    tmux_sessions: Vec<TmuxSession>,
    selected_tmux: Option<String>,
    tmux_new_name: String,
    tmux_notice: String,
    recent_errors: SanitizedErrorHistory,
    last_recorded_error: String,
    support_report: String,
    support_export_path: String,
    support_notice: String,
    known_hosts_path: String,
    host_key_notice: String,
    host_key_remove_confirm: Option<(terminal::HostKeyTarget, Option<PathBuf>)>,
    error: String,
    writable: bool,
    tabs: Vec<Tab>,
    active: Option<u64>,
    terminal_focus: Option<u64>,
    paste_policy: PastePolicy,
    pending_paste: Option<(u64, String)>,
    paste_notice: String,
    search_open: bool,
    search_query: String,
    logging_tab: Option<u64>,
    session_log_path: String,
    last_logged_screen: String,
    log_notice: String,
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
            profile_transfer_path: String::new(),
            replace_import_confirm: None,
            profile_transfer_notice: String::new(),
            draft: Session::default(),
            port: String::new(),
            proxy_port: String::new(),
            control_persist: String::new(),
            control_master_notice: String::new(),
            connect_timeout: String::new(),
            keepalive: String::new(),
            local_forwards: String::new(),
            remote_forwards: String::new(),
            dynamic_forwards: String::new(),
            forward_risk_ack: false,
            tunnel_process: None,
            tunnel_notice: String::new(),
            sftp_browser: None,
            scp_panel: None,
            tmux_sessions: Vec::new(),
            selected_tmux: None,
            tmux_new_name: String::new(),
            tmux_notice: String::new(),
            recent_errors: SanitizedErrorHistory::default(),
            last_recorded_error: String::new(),
            support_report: String::new(),
            support_export_path: String::new(),
            support_notice: String::new(),
            known_hosts_path: String::new(),
            host_key_notice: String::new(),
            host_key_remove_confirm: None,
            error,
            writable,
            tabs: Vec::new(),
            active: None,
            terminal_focus: None,
            paste_policy: PastePolicy::default(),
            pending_paste: None,
            paste_notice: String::new(),
            search_open: false,
            search_query: String::new(),
            logging_tab: None,
            session_log_path: String::new(),
            last_logged_screen: String::new(),
            log_notice: String::new(),
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
        self.proxy_port = session
            .ssh
            .proxy_port
            .map(|value| value.to_string())
            .unwrap_or_default();
        self.control_persist = session
            .ssh
            .control_persist_seconds
            .map(|value| value.to_string())
            .unwrap_or_default();
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
        self.forward_risk_ack = false;
        self.tunnel_notice.clear();
        self.tmux_sessions.clear();
        self.selected_tmux = None;
        self.tmux_new_name.clear();
        self.tmux_notice.clear();
        self.support_report.clear();
        self.support_notice.clear();
        self.draft = session;
    }

    fn profile_transfer_path(&self) -> anyhow::Result<PathBuf> {
        let value = self.profile_transfer_path.trim();
        anyhow::ensure!(!value.is_empty(), "profile import/export path is required");
        Ok(PathBuf::from(value))
    }

    fn export_profile_library(&self) -> anyhow::Result<String> {
        let path = self.profile_transfer_path()?;
        export_sessions(&path, &self.profiles)?;
        Ok(format!(
            "Exported {} profile(s) to {}",
            self.profiles.len(),
            path.display()
        ))
    }

    fn import_profile_library(
        &mut self,
        path: &std::path::Path,
        mode: SessionImportMode,
    ) -> anyhow::Result<String> {
        let next = import_sessions(path, &self.profiles, mode)?;
        let imported_count = match mode {
            SessionImportMode::Merge => next.len().saturating_sub(self.profiles.len()),
            SessionImportMode::Replace => next.len(),
        };

        // Persist the fully validated candidate before changing in-memory state.
        save_sessions(&self.path, &next)?;
        self.profiles = next;
        self.delete_confirm = None;
        self.replace_import_confirm = None;

        if mode == SessionImportMode::Replace {
            self.selected_profile = None;
            self.load_draft(Session::default());
        }

        Ok(match mode {
            SessionImportMode::Merge => {
                format!("Merged {imported_count} profile(s) from {}", path.display())
            }
            SessionImportMode::Replace => format!(
                "Replaced the saved library with {imported_count} profile(s) from {}",
                path.display()
            ),
        })
    }

    fn known_hosts_override(&self) -> Option<PathBuf> {
        let value = self.known_hosts_path.trim();
        (!value.is_empty()).then(|| PathBuf::from(value))
    }

    fn inspect_draft_host_key(&mut self) -> anyhow::Result<()> {
        let session = self.validated_draft()?;
        let target = terminal::resolve_host_key_target(&session, self.config.as_deref())?;
        let known_hosts = self.known_hosts_override();
        let found = terminal::inspect_known_host(&target, known_hosts.as_deref())?;
        self.host_key_notice = if found.is_empty() {
            format!(
                "No matching key found for {:?}{}.",
                target.lookup,
                known_hosts
                    .as_ref()
                    .map(|path| format!(" in {}", path.display()))
                    .unwrap_or_else(|| " in ssh-keygen's default known_hosts file".into())
            )
        } else {
            format!("Effective host-key target: {:?}\n{}", target.lookup, found)
        };
        self.host_key_remove_confirm = None;
        Ok(())
    }

    fn prepare_host_key_removal(&mut self) -> anyhow::Result<()> {
        let session = self.validated_draft()?;
        let target = terminal::resolve_host_key_target(&session, self.config.as_deref())?;
        self.host_key_remove_confirm = Some((target, self.known_hosts_override()));
        Ok(())
    }

    fn validated_draft(&self) -> anyhow::Result<Session> {
        let mut session = self.draft.clone();
        session.port = parse_optional_u16(&self.port, "port")?;
        session.ssh.proxy_port = parse_optional_u16(&self.proxy_port, "proxy port")?;
        session.ssh.control_persist_seconds = if self.control_persist.trim().is_empty() {
            None
        } else {
            Some(self.control_persist.trim().parse::<u32>().map_err(|_| {
                anyhow::anyhow!("ControlPersist must be a positive whole number of seconds")
            })?)
        };
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

        let active_terminal = self
            .active
            .filter(|id| self.terminal_focus == Some(*id))
            .and_then(|id| {
                self.tabs
                    .iter()
                    .find(|tab| tab.id == id && !tab.exited)
                    .map(|_| id)
            });

        if let Some(id) = active_terminal {
            let policy = self.paste_policy;
            let mut confirm_payload = None;
            let mut blocked = false;
            ctx.input_mut(|input| {
                input.events.retain(|event| {
                    let egui::Event::Paste(text) = event else {
                        return true;
                    };
                    match terminal_ux::classify_paste(policy, text) {
                        PasteDecision::Send => true,
                        PasteDecision::Confirm => {
                            if confirm_payload.is_none() {
                                confirm_payload = Some(text.clone());
                            }
                            false
                        }
                        PasteDecision::Block => {
                            blocked = true;
                            false
                        }
                    }
                });
            });
            if let Some(text) = confirm_payload {
                self.pending_paste = Some((id, text));
                self.terminal_focus = None;
                self.paste_notice = "Multiline paste is waiting for explicit confirmation.".into();
            } else if blocked {
                self.paste_notice =
                    "Paste blocked by policy (multiline or NUL-containing payload).".into();
            }
        }

        let search_shortcut = ctx.input(|input| {
            input.key_pressed(egui::Key::F)
                && input.modifiers.command
                && input.modifiers.shift
        });
        if search_shortcut {
            self.search_open = true;
            self.terminal_focus = None;
        }
        if self.search_open && ctx.input(|input| input.key_pressed(egui::Key::Escape)) {
            self.search_open = false;
            self.terminal_focus = self.active;
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

                    egui::CollapsingHeader::new("Profile import / export")
                        .default_open(false)
                        .show(ui, |ui| {
                            ui.small(
                                "Uses Inspirum's validated non-secret JSON profile format. Hostnames, usernames, identity-file paths and SSH policy settings are included; passwords, passphrases and private-key contents are not.",
                            );
                            ui.label("JSON file path");
                            if ui
                                .add(
                                    egui::TextEdit::singleline(&mut self.profile_transfer_path)
                                        .hint_text("/path/to/inspirum-profiles.json"),
                                )
                                .has_focus()
                            {
                                self.terminal_focus = None;
                            }
                            ui.small(
                                "Identity-file paths and other local paths may need adjustment when importing on another machine.",
                            );

                            ui.horizontal_wrapped(|ui| {
                                if ui
                                    .add_enabled(
                                        self.writable,
                                        egui::Button::new("Export all"),
                                    )
                                    .clicked()
                                {
                                    self.terminal_focus = None;
                                    match self.export_profile_library() {
                                        Ok(message) => {
                                            self.error.clear();
                                            self.profile_transfer_notice = message;
                                        }
                                        Err(error) => {
                                            self.profile_transfer_notice.clear();
                                            self.error = format!("{error:#}");
                                        }
                                    }
                                }

                                if ui
                                    .add_enabled(
                                        self.writable,
                                        egui::Button::new("Import + merge"),
                                    )
                                    .on_hover_text(
                                        "Reject the entire import if any imported profile name already exists",
                                    )
                                    .clicked()
                                {
                                    self.terminal_focus = None;
                                    let result = self
                                        .profile_transfer_path()
                                        .and_then(|path| {
                                            self.import_profile_library(
                                                &path,
                                                SessionImportMode::Merge,
                                            )
                                        });
                                    match result {
                                        Ok(message) => {
                                            self.error.clear();
                                            self.profile_transfer_notice = message;
                                        }
                                        Err(error) => {
                                            self.profile_transfer_notice.clear();
                                            self.error = format!("{error:#}");
                                        }
                                    }
                                }

                                if ui
                                    .add_enabled(
                                        self.writable,
                                        egui::Button::new("Replace from file"),
                                    )
                                    .clicked()
                                {
                                    self.terminal_focus = None;
                                    match self.profile_transfer_path() {
                                        Ok(path) => {
                                            self.error.clear();
                                            self.profile_transfer_notice.clear();
                                            self.replace_import_confirm = Some(path);
                                        }
                                        Err(error) => {
                                            self.profile_transfer_notice.clear();
                                            self.error = format!("{error:#}");
                                        }
                                    }
                                }
                            });

                            if let Some(confirmed_path) = self.replace_import_confirm.clone() {
                                ui.group(|ui| {
                                    ui.label(format!(
                                        "Replace every saved profile with the validated contents of {}?",
                                        confirmed_path.display()
                                    ));
                                    ui.small(
                                        "Open SSH/SFTP tabs stay connected. The active profile file is changed only after the complete import validates and the atomic save succeeds. Editing the path field above does not change this confirmation.",
                                    );
                                    ui.horizontal(|ui| {
                                        if ui.button("Confirm replace").clicked() {
                                            match self.import_profile_library(
                                                &confirmed_path,
                                                SessionImportMode::Replace,
                                            ) {
                                                Ok(message) => {
                                                    self.error.clear();
                                                    self.profile_transfer_notice = message;
                                                }
                                                Err(error) => {
                                                    self.profile_transfer_notice.clear();
                                                    self.error = format!("{error:#}");
                                                }
                                            }
                                        }
                                        if ui.button("Cancel").clicked() {
                                            self.replace_import_confirm = None;
                                        }
                                    });
                                });
                            }

                            if !self.profile_transfer_notice.is_empty() {
                                ui.small(&self.profile_transfer_notice);
                            }
                        });

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

                    egui::CollapsingHeader::new("Host key trust")
                        .default_open(false)
                        .show(ui, |ui| {
                            ui.strong(if self.draft.strict {
                                "Effective app policy: already-trusted keys only"
                            } else {
                                "Effective app policy: ask before trusting a new key"
                            });
                            ui.small(
                                "Inspirum never auto-accepts host keys. Verify a new fingerprint through an independent trusted channel.",
                            );
                            ui.label("known_hosts file override (blank: ssh-keygen default)");
                            if ui
                                .add(
                                    egui::TextEdit::singleline(&mut self.known_hosts_path)
                                        .hint_text("/path/to/known_hosts"),
                                )
                                .has_focus()
                            {
                                self.terminal_focus = None;
                            }
                            ui.small(
                                "Use an explicit file here when SSH config uses a custom UserKnownHostsFile. The file path is not saved in the profile.",
                            );

                            ui.horizontal_wrapped(|ui| {
                                if ui.button("Inspect trusted key").clicked() {
                                    self.terminal_focus = None;
                                    if let Err(error) = self.inspect_draft_host_key() {
                                        self.host_key_notice.clear();
                                        self.error = format!("{error:#}");
                                    } else {
                                        self.error.clear();
                                    }
                                }
                                if ui
                                    .button("Remove trusted key…")
                                    .on_hover_text(
                                        "Resolve the effective OpenSSH host identity, then ask for confirmation before editing known_hosts",
                                    )
                                    .clicked()
                                {
                                    self.terminal_focus = None;
                                    if let Err(error) = self.prepare_host_key_removal() {
                                        self.error = format!("{error:#}");
                                    } else {
                                        self.error.clear();
                                    }
                                }
                            });

                            if let Some((target, known_hosts)) =
                                self.host_key_remove_confirm.clone()
                            {
                                ui.group(|ui| {
                                    let file = known_hosts
                                        .as_ref()
                                        .map(|path| path.display().to_string())
                                        .unwrap_or_else(|| {
                                            "ssh-keygen's default known_hosts file".into()
                                        });
                                    ui.label(format!(
                                        "Remove all trusted keys matching {:?} from {file}?",
                                        target.lookup
                                    ));
                                    ui.small(format!(
                                        "Resolved destination: {}:{}{}",
                                        target.hostname,
                                        target.port,
                                        target
                                            .host_key_alias
                                            .as_ref()
                                            .map(|alias| format!(", HostKeyAlias={alias}"))
                                            .unwrap_or_default()
                                    ));
                                    ui.small(
                                        "Removal does not trust a replacement key. Ask mode will prompt on a later connection; strict mode will reject until an appropriate key is trusted.",
                                    );
                                    ui.horizontal(|ui| {
                                        if ui.button("Confirm removal").clicked() {
                                            match terminal::remove_known_host(
                                                &target,
                                                known_hosts.as_deref(),
                                            ) {
                                                Ok(message) => {
                                                    self.host_key_notice = if message.is_empty() {
                                                        format!(
                                                            "Removed trusted-key entries for {:?}.",
                                                            target.lookup
                                                        )
                                                    } else {
                                                        message
                                                    };
                                                    self.host_key_remove_confirm = None;
                                                    self.error.clear();
                                                }
                                                Err(error) => {
                                                    self.error = format!("{error:#}");
                                                }
                                            }
                                        }
                                        if ui.button("Cancel").clicked() {
                                            self.host_key_remove_confirm = None;
                                        }
                                    });
                                });
                            }

                            if !self.host_key_notice.is_empty() {
                                ui.separator();
                                ui.monospace(&self.host_key_notice);
                            }
                        });

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
                            ui.strong("Structured proxy");
                            ui.horizontal_wrapped(|ui| {
                                ui.label("Transport");
                                if ui
                                    .selectable_value(
                                        &mut self.draft.ssh.proxy_kind,
                                        ProxyKind::None,
                                        "None",
                                    )
                                    .changed()
                                    || ui
                                        .selectable_value(
                                            &mut self.draft.ssh.proxy_kind,
                                            ProxyKind::HttpConnect,
                                            "HTTP CONNECT",
                                        )
                                        .changed()
                                    || ui
                                        .selectable_value(
                                            &mut self.draft.ssh.proxy_kind,
                                            ProxyKind::Socks5,
                                            "SOCKS5",
                                        )
                                        .changed()
                                {
                                    self.terminal_focus = None;
                                }
                            });
                            if self.draft.ssh.proxy_kind != ProxyKind::None {
                                ui.label("Proxy host");
                                if ui
                                    .text_edit_singleline(&mut self.draft.ssh.proxy_host)
                                    .has_focus()
                                {
                                    self.terminal_focus = None;
                                }
                                ui.label("Proxy port");
                                if ui.text_edit_singleline(&mut self.proxy_port).has_focus() {
                                    self.terminal_focus = None;
                                }
                                ui.small(
                                    "ProxyJump and structured proxy transport cannot be enabled together. HTTP/SOCKS proxy authentication is not stored or supported yet.",
                                );
                                ui.small(
                                    "Inspirum supplies a built-in transport helper to OpenSSH ProxyCommand. If the proxy fails or denies the tunnel, the SSH connection fails; it does not retry directly.",
                                );
                            }

                            ui.separator();
                            ui.strong("Connection multiplexing");
                            ui.horizontal_wrapped(|ui| {
                                ui.label("ControlMaster");
                                ui.selectable_value(&mut self.draft.ssh.control_master, ControlMasterMode::Inherit, "Inherit");
                                ui.selectable_value(&mut self.draft.ssh.control_master, ControlMasterMode::Disabled, "Disabled");
                                ui.selectable_value(&mut self.draft.ssh.control_master, ControlMasterMode::Auto, "Auto");
                            });
                            if self.draft.ssh.control_master == ControlMasterMode::Auto {
                                ui.label("ControlPath");
                                if ui.text_edit_singleline(&mut self.draft.ssh.control_path).has_focus() { self.terminal_focus = None; }
                                ui.label("ControlPersist seconds (optional)");
                                if ui.text_edit_singleline(&mut self.control_persist).has_focus() { self.terminal_focus = None; }
                                ui.horizontal(|ui| {
                                    if ui.button("Check master").clicked() {
                                        self.control_master_notice = match self.validated_draft().and_then(|s| terminal::control_master_operation(&s, self.config.as_deref(), "check")) { Ok(v) => if v.is_empty() { "Master is active.".into() } else { v }, Err(e) => format!("{e:#}") };
                                    }
                                    if ui.button("Close master").clicked() {
                                        self.control_master_notice = match self.validated_draft().and_then(|s| terminal::control_master_operation(&s, self.config.as_deref(), "exit")) { Ok(v) => if v.is_empty() { "Master close requested.".into() } else { v }, Err(e) => format!("{e:#}") };
                                    }
                                });
                                if !self.control_master_notice.is_empty() { ui.small(&self.control_master_notice); }
                                ui.small("Auto uses OpenSSH ControlMaster=auto with this explicit ControlPath. Stale or unavailable sockets are surfaced; Inspirum never silently deletes them.");
                            } else {
                                ui.small("Inherit leaves ~/.ssh/config multiplexing untouched. Disabled passes ControlMaster=no.");
                            }

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
                            ui.strong("tmux sessions");
                            ui.small(
                                "tmux integration is explicit: Refresh discovers sessions, Attach selects an existing one, and Create starts only the name you enter. Ordinary SSH Connect never attaches or creates tmux automatically.",
                            );
                            ui.horizontal_wrapped(|ui| {
                                if ui.button("Refresh tmux").clicked() {
                                    self.terminal_focus = None;
                                    let result = self.validated_draft().and_then(|session| {
                                        let sessions = tmux::list_sessions(
                                            &session,
                                            self.config.as_deref(),
                                        )?;
                                        if !self
                                            .selected_tmux
                                            .as_ref()
                                            .is_some_and(|selected| {
                                                sessions.iter().any(|item| &item.name == selected)
                                            })
                                        {
                                            self.selected_tmux =
                                                sessions.first().map(|item| item.name.clone());
                                        }
                                        self.tmux_notice =
                                            format!("Found {} tmux session(s).", sessions.len());
                                        self.tmux_sessions = sessions;
                                        Ok(())
                                    });
                                    self.error =
                                        result.err().map(|e| format!("{e:#}")).unwrap_or_default();
                                }
                                if !self.tmux_sessions.is_empty() {
                                    egui::ComboBox::from_id_salt("tmux-session-select")
                                        .selected_text(
                                            self.selected_tmux
                                                .as_deref()
                                                .unwrap_or("Select tmux session"),
                                        )
                                        .show_ui(ui, |ui| {
                                            for item in &self.tmux_sessions {
                                                let label = format!(
                                                    "{} ({} attached)",
                                                    item.name, item.attached_clients
                                                );
                                                ui.selectable_value(
                                                    &mut self.selected_tmux,
                                                    Some(item.name.clone()),
                                                    label,
                                                );
                                            }
                                        });
                                }
                            });
                            ui.horizontal_wrapped(|ui| {
                                let can_open = self.tabs.len() < 16;
                                let can_attach = can_open && self.selected_tmux.is_some();
                                if ui
                                    .add_enabled(can_attach, egui::Button::new("Attach selected"))
                                    .clicked()
                                {
                                    self.terminal_focus = None;
                                    let selected = self.selected_tmux.clone().unwrap_or_default();
                                    let result = self.validated_draft().and_then(|session| {
                                        let attach = tmux::attach_session(&session, &selected)?;
                                        let terminal = terminal::connect(
                                            self.next_id,
                                            ctx.clone(),
                                            self.tx.clone(),
                                            &attach,
                                            self.config.as_deref(),
                                        )?;
                                        self.tabs.push(Tab {
                                            id: self.next_id,
                                            name: format!("{} · tmux:{selected}", session.name),
                                            kind: TabKind::Ssh,
                                            session: attach,
                                            terminal,
                                            exited: false,
                                        });
                                        self.active = Some(self.next_id);
                                        self.terminal_focus = Some(self.next_id);
                                        self.next_id += 1;
                                        self.tmux_notice =
                                            format!("Attaching tmux session {selected:?}.");
                                        Ok(())
                                    });
                                    self.error =
                                        result.err().map(|e| format!("{e:#}")).unwrap_or_default();
                                }
                                ui.label("New");
                                if ui.text_edit_singleline(&mut self.tmux_new_name).has_focus() {
                                    self.terminal_focus = None;
                                }
                                if ui
                                    .add_enabled(can_open, egui::Button::new("Create tmux"))
                                    .clicked()
                                {
                                    self.terminal_focus = None;
                                    let name = self.tmux_new_name.trim().to_owned();
                                    let result = self.validated_draft().and_then(|session| {
                                        let create = tmux::create_session(&session, &name)?;
                                        let reconnect = tmux::attach_session(&session, &name)?;
                                        let terminal = terminal::connect(
                                            self.next_id,
                                            ctx.clone(),
                                            self.tx.clone(),
                                            &create,
                                            self.config.as_deref(),
                                        )?;
                                        self.tabs.push(Tab {
                                            id: self.next_id,
                                            name: format!("{} · tmux:{name}", session.name),
                                            kind: TabKind::Ssh,
                                            session: reconnect,
                                            terminal,
                                            exited: false,
                                        });
                                        self.active = Some(self.next_id);
                                        self.terminal_focus = Some(self.next_id);
                                        self.next_id += 1;
                                        self.selected_tmux = Some(name.clone());
                                        self.tmux_notice =
                                            format!("Creating tmux session {name:?}.");
                                        Ok(())
                                    });
                                    self.error =
                                        result.err().map(|e| format!("{e:#}")).unwrap_or_default();
                                }
                            });
                            ui.small(
                                "Refresh uses non-interactive OpenSSH authentication (agent/key/ControlMaster). Create/Attach use the normal SSH PTY. Closing a tmux tab disconnects that tmux client and leaves the server-side tmux session running.",
                            );
                            if !self.tmux_notice.is_empty() {
                                ui.small(&self.tmux_notice);
                            }

                            ui.separator();
                            ui.strong("Port forwarding");
                            ui.small(
                                "One OpenSSH forwarding specification per line. These are added to any forwards from OpenSSH config. Profile forwarding is tied to this SSH session and fails the connection if setup fails.",
                            );

                            if forward_editor(
                                ui,
                                "Local forward (-L)",
                                &mut self.local_forwards,
                                "127.0.0.1:8080:internal.example:80",
                            ) {
                                self.terminal_focus = None;
                            }
                            if forward_editor(
                                ui,
                                "Remote forward (-R)",
                                &mut self.remote_forwards,
                                "127.0.0.1:9000:127.0.0.1:3000",
                            ) {
                                self.terminal_focus = None;
                            }
                            if forward_editor(
                                ui,
                                "Dynamic forward (-D)",
                                &mut self.dynamic_forwards,
                                "127.0.0.1:1080",
                            ) {
                                self.terminal_focus = None;
                            }

                            let preview = self.validated_draft().ok();
                            let exposed = preview
                                .as_ref()
                                .is_some_and(crate::session_requires_forward_risk_ack);
                            if exposed {
                                ui.checkbox(
                                    &mut self.forward_risk_ack,
                                    "I understand one or more listeners bind beyond loopback",
                                );
                                ui.small(
                                    "Non-loopback listeners may expose local or remote services to other hosts. This acknowledgement is required each time the tunnel manager is started.",
                                );
                            }

                            ui.horizontal(|ui| {
                                let running = self
                                    .tunnel_process
                                    .as_mut()
                                    .is_some_and(|process| process.is_running().unwrap_or(false));
                                if ui
                                    .add_enabled(!running, egui::Button::new("Start tunnels"))
                                    .clicked()
                                {
                                    let result = self.validated_draft().and_then(|session| {
                                        anyhow::ensure!(
                                            !crate::session_requires_forward_risk_ack(&session)
                                                || self.forward_risk_ack,
                                            "non-loopback tunnel binds require explicit risk acknowledgement"
                                        );
                                        let process = terminal::start_tunnels(
                                            &session,
                                            self.config.as_deref(),
                                        )?;
                                        let count = session.ssh.local_forwards.len()
                                            + session.ssh.remote_forwards.len()
                                            + session.ssh.dynamic_forwards.len();
                                        self.tunnel_notice = format!(
                                            "{count} tunnel(s) running in SSH process {}",
                                            process.id()
                                        );
                                        self.tunnel_process = Some(process);
                                        Ok(())
                                    });
                                    self.error =
                                        result.err().map(|e| format!("{e:#}")).unwrap_or_default();
                                }
                                if ui
                                    .add_enabled(running, egui::Button::new("Stop tunnels"))
                                    .clicked()
                                    && let Some(mut process) = self.tunnel_process.take()
                                {
                                    match process.stop() {
                                        Ok(()) => self.tunnel_notice = "Tunnels stopped; listeners closed.".into(),
                                        Err(error) => self.error = format!("{error:#}"),
                                    }
                                }
                            });
                            if !self.tunnel_notice.is_empty() {
                                ui.small(&self.tunnel_notice);
                            }
                            if let Some(session) = preview {
                                let status = if self
                                    .tunnel_process
                                    .as_mut()
                                    .is_some_and(|process| process.is_running().unwrap_or(false))
                                {
                                    "running"
                                } else {
                                    "stopped"
                                };
                                for spec in &session.ssh.local_forwards {
                                    ui.small(format!("Local  {spec}  • {status}"));
                                }
                                for spec in &session.ssh.remote_forwards {
                                    ui.small(format!("Remote {spec}  • {status}"));
                                }
                                for spec in &session.ssh.dynamic_forwards {
                                    ui.small(format!("SOCKS  {spec}  • {status}"));
                                }
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
                        if ui
                            .button("Files")
                            .on_hover_text("Open the graphical SFTP browser and transfer queue")
                            .clicked()
                        {
                            self.terminal_focus = None;
                            let result = self.validated_draft().and_then(|session| {
                                self.sftp_browser = Some(SftpBrowser::new(
                                    session,
                                    self.config.clone(),
                                )?);
                                Ok(())
                            });
                            self.error =
                                result.err().map(|e| format!("{e:#}")).unwrap_or_default();
                        }
                        if ui
                            .button("SCP")
                            .on_hover_text("Open explicit SCP upload/download operations")
                            .clicked()
                        {
                            self.terminal_focus = None;
                            let result = self.validated_draft().map(|session| {
                                self.scp_panel = Some(ScpPanel::new(
                                    session,
                                    self.config.clone(),
                                ));
                            });
                            self.error =
                                result.err().map(|e| format!("{e:#}")).unwrap_or_default();
                        }
                    });
                    ui.small(
                        "SFTP reuses host trust, OpenSSH config, identity, ProxyJump, timeout, keepalive and compression. Terminal-only remote commands, X11/agent forwarding and port forwards are not applied to SFTP.",
                    );

                    egui::CollapsingHeader::new("Support diagnostics")
                        .default_open(false)
                        .show(ui, |ui| {
                            ui.small(
                                "Generates a privacy-safe support report from local OpenSSH capability probes, allowlisted launch-policy state, platform metadata and sanitized in-memory error categories. Raw connection errors and arbitrary environment variables are never included.",
                            );
                            ui.horizontal_wrapped(|ui| {
                                if ui.button("Generate support report").clicked() {
                                    self.terminal_focus = None;
                                    let result = self.validated_draft().map(|session| {
                                        self.support_report = support::collect(
                                            Some(&session),
                                            self.config.is_some(),
                                            &self.recent_errors,
                                        );
                                        self.support_notice = format!(
                                            "Support report generated with {} sanitized recent error(s).",
                                            self.recent_errors.len()
                                        );
                                    });
                                    self.error =
                                        result.err().map(|e| format!("{e:#}")).unwrap_or_default();
                                }
                                if ui
                                    .add_enabled(
                                        !self.recent_errors.is_empty(),
                                        egui::Button::new("Clear recent errors"),
                                    )
                                    .clicked()
                                {
                                    self.recent_errors.clear();
                                    self.last_recorded_error.clear();
                                    self.support_notice =
                                        "Sanitized in-memory error history cleared.".into();
                                }
                            });
                            ui.label("Export path");
                            if ui
                                .text_edit_singleline(&mut self.support_export_path)
                                .has_focus()
                            {
                                self.terminal_focus = None;
                            }
                            if ui
                                .add_enabled(
                                    !self.support_report.is_empty(),
                                    egui::Button::new("Export support report"),
                                )
                                .clicked()
                            {
                                let path = PathBuf::from(self.support_export_path.trim());
                                let result = (|| -> anyhow::Result<()> {
                                    anyhow::ensure!(
                                        !self.support_export_path.trim().is_empty(),
                                        "support export path is required"
                                    );
                                    support::export(&path, &self.support_report)?;
                                    self.support_notice =
                                        "Support report exported without overwriting an existing file."
                                            .into();
                                    Ok(())
                                })();
                                self.error =
                                    result.err().map(|e| format!("{e:#}")).unwrap_or_default();
                            }
                            if !self.support_notice.is_empty() {
                                ui.small(&self.support_notice);
                            }
                            if !self.support_report.is_empty() {
                                ui.add(
                                    egui::TextEdit::multiline(&mut self.support_report)
                                        .desired_rows(14)
                                        .font(egui::TextStyle::Monospace)
                                        .interactive(false),
                                );
                            }
                        });

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
                    let close_help = if tab
                        .session
                        .ssh
                        .remote_command
                        .starts_with("exec tmux attach-session -t ")
                    {
                        "Disconnect/detach this tmux client and close the terminal; the server-side tmux session remains"
                    } else {
                        "Disconnect and close terminal"
                    };
                    if ui
                        .small_button("×")
                        .on_hover_text(close_help)
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
        let mut close_browser = false;
        let mut close_scp = false;
        let mut copy_selected = false;
        let active_screen = if self.search_open || self.logging_tab.is_some() {
            self.active.and_then(|id| {
                self.tabs
                    .iter_mut()
                    .find(|tab| tab.id == id)
                    .map(|tab| terminal_screen_text(&mut tab.terminal))
            })
        } else {
            None
        };
        let search_hits = active_screen
            .as_deref()
            .map(|screen| terminal_ux::find_text(screen, &self.search_query, 50))
            .unwrap_or_default();

        egui::CentralPanel::default().show(ctx, |ui| {
            if let Some(panel) = self.scp_panel.as_mut() {
                ui.horizontal(|ui| {
                    ui.strong(format!("SCP · {}", panel.session_name()));
                    if ui.button("Close SCP").clicked() {
                        close_scp = true;
                    }
                });
                ui.separator();
                panel.ui(ctx, ui);
                return;
            }
            if let Some(browser) = self.sftp_browser.as_mut() {
                ui.horizontal(|ui| {
                    ui.strong(format!("Files · {}", browser.session_name()));
                    if ui.button("Close file browser").clicked() {
                        close_browser = true;
                    }
                });
                ui.separator();
                browser.ui(ctx, ui);
                return;
            }
            if self.sftp_browser.is_none() && self.scp_panel.is_none() && self.active.is_some() {
                ui.horizontal_wrapped(|ui| {
                    if ui.button("Copy selection").clicked() {
                        copy_selected = true;
                    }
                    if ui
                        .button(if self.search_open { "Close search" } else { "Search" })
                        .on_hover_text("Keyboard: Ctrl/Cmd+Shift+F")
                        .clicked()
                    {
                        self.search_open = !self.search_open;
                        self.terminal_focus = if self.search_open { None } else { self.active };
                    }

                    ui.label("Paste:");
                    ui.selectable_value(
                        &mut self.paste_policy,
                        PastePolicy::ConfirmMultiline,
                        "confirm multiline",
                    );
                    ui.selectable_value(
                        &mut self.paste_policy,
                        PastePolicy::ConfirmAll,
                        "confirm all",
                    );
                    ui.selectable_value(
                        &mut self.paste_policy,
                        PastePolicy::BlockMultiline,
                        "block multiline",
                    );
                });

                if !self.paste_notice.is_empty() {
                    ui.small(&self.paste_notice);
                }

                if self.search_open {
                    ui.horizontal(|ui| {
                        ui.label("Find");
                        let response = ui.add(
                            egui::TextEdit::singleline(&mut self.search_query)
                                .hint_text("Search current terminal viewport"),
                        );
                        if response.has_focus() {
                            self.terminal_focus = None;
                        }
                        ui.label(format!("{} match(es)", search_hits.len()));
                    });
                    egui::ScrollArea::vertical()
                        .max_height(110.0)
                        .show(ui, |ui| {
                            for hit in &search_hits {
                                ui.monospace(format!(
                                    "{}:{}  {}",
                                    hit.line, hit.column, hit.preview
                                ));
                            }
                        });
                }

                ui.separator();
                ui.horizontal_wrapped(|ui| {
                    ui.label("Session log");
                    let path_edit = ui.add(
                        egui::TextEdit::singleline(&mut self.session_log_path)
                            .desired_width(280.0)
                            .hint_text("new log file path"),
                    );
                    if path_edit.has_focus() {
                        self.terminal_focus = None;
                    }
                    if self.logging_tab.is_none() {
                        if ui
                            .add_enabled(
                                self.active.is_some() && !self.session_log_path.trim().is_empty(),
                                egui::Button::new("Start logging"),
                            )
                            .on_hover_text(
                                "Opt-in screen snapshots. Terminal output can contain passwords, tokens and other secrets.",
                            )
                            .clicked()
                        {
                            let path = PathBuf::from(self.session_log_path.trim());
                            match terminal_ux::start_session_log(&path) {
                                Ok(()) => {
                                    self.logging_tab = self.active;
                                    self.last_logged_screen.clear();
                                    self.log_notice =
                                        "Logging enabled for the active tab. Input keystrokes are not logged; visible output may contain secrets.".into();
                                }
                                Err(error) => {
                                    self.log_notice = format!("{error:#}");
                                }
                            }
                        }
                    } else if ui.button("Stop logging").clicked() {
                        self.logging_tab = None;
                        self.last_logged_screen.clear();
                        self.log_notice = "Session logging stopped.".into();
                    }
                });
                if !self.log_notice.is_empty() {
                    ui.small(&self.log_notice);
                }
                ui.separator();
            }

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
                if copy_selected {
                    let selected = tab.terminal.selectable_content();
                    if !selected.is_empty() {
                        ctx.copy_text(selected);
                    }
                }
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
        if let Some((id, text)) = self.pending_paste.clone() {
            let mut confirm = false;
            let mut cancel = false;
            egui::Window::new("Confirm terminal paste")
                .collapsible(false)
                .resizable(true)
                .show(ctx, |ui| {
                    let line_count = text
                        .as_bytes()
                        .iter()
                        .filter(|byte| matches!(byte, b'\r' | b'\n'))
                        .count()
                        + 1;
                    ui.strong(format!(
                        "Paste {} bytes across approximately {} line(s)?",
                        text.len(),
                        line_count
                    ));
                    ui.label(
                        "Review carefully. Multiline terminal pastes can execute several commands immediately.",
                    );
                    let preview = text.chars().take(4000).collect::<String>();
                    ui.add(
                        egui::TextEdit::multiline(&mut preview.as_str())
                            .desired_rows(8)
                            .interactive(false),
                    );
                    ui.horizontal(|ui| {
                        if ui.button("Paste now").clicked() {
                            confirm = true;
                        }
                        if ui.button("Cancel").clicked() {
                            cancel = true;
                        }
                    });
                    ui.small("Keyboard: Enter confirms; Escape cancels.");
                });
            confirm |= ctx.input(|input| input.key_pressed(egui::Key::Enter));
            cancel |= ctx.input(|input| input.key_pressed(egui::Key::Escape));
            if confirm {
                if let Some(tab) = self.tabs.iter_mut().find(|tab| tab.id == id && !tab.exited) {
                    tab.terminal
                        .process_command(BackendCommand::Write(text.into_bytes()));
                    self.paste_notice = "Paste sent after explicit confirmation.".into();
                    self.terminal_focus = Some(id);
                } else {
                    self.paste_notice = "Paste cancelled because the terminal is no longer active.".into();
                }
                self.pending_paste = None;
            } else if cancel {
                self.pending_paste = None;
                self.paste_notice = "Paste cancelled.".into();
                self.terminal_focus = self.active;
            }
        }

        if let Some(log_id) = self.logging_tab {
            if let Some(tab) = self.tabs.iter_mut().find(|tab| tab.id == log_id) {
                let screen = terminal_screen_text(&mut tab.terminal);
                let path = PathBuf::from(self.session_log_path.trim());
                if let Err(error) = terminal_ux::append_screen_snapshot(
                    &path,
                    &mut self.last_logged_screen,
                    &screen,
                ) {
                    self.log_notice = format!("Session logging stopped: {error:#}");
                    self.logging_tab = None;
                    self.last_logged_screen.clear();
                }
            } else {
                self.logging_tab = None;
                self.last_logged_screen.clear();
                self.log_notice = "Session logging stopped because its tab was closed.".into();
            }
        }

        if close_browser {
            self.sftp_browser = None;
        }
        if close_scp {
            self.scp_panel = None;
        }

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

        if self.error.is_empty() {
            self.last_recorded_error.clear();
        } else if self.error != self.last_recorded_error {
            self.recent_errors.record(&self.error);
            self.last_recorded_error = self.error.clone();
        }
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.ui(ctx);
    }
}
