//! Small native connection/profile interface; terminal mechanics stay upstream.
use crate::{
    ControlMasterMode, ProxyKind, Session, SessionImportMode,
    appearance::{
        self, AppearanceOverride, AppearanceSettings, TerminalAppearance, TerminalCursorStyle,
        TerminalFontFamily, TerminalPalette,
    },
    delete_session, duplicate_session_draft, export_sessions,
    history::{HistoryRow, HistoryState},
    import_sessions,
    keyboard::{self, ShortcutAction, ShortcutKey, ShortcutModifiers},
    load_sessions, save_session_edit, save_sessions,
    scp_panel::ScpPanel,
    session_matches_query, session_profile_key,
    sftp_browser::SftpBrowser,
    startup::{self, StartupBehavior, StartupSettings},
    support::{self, SanitizedErrorHistory},
    tab_management::{self, BulkCloseMode, TabVisualLabel},
    terminal,
    terminal_ux::{self, PasteDecision, PastePolicy},
    tmux::{self, TmuxSession},
    workspace::{self, SplitAxis, SyncInputState, WorkspaceLayout},
};
use eframe::egui;
use egui_term::{
    BackendCommand, ColorPalette, CursorStyle, FontSettings, InteractionSettings, PtyEvent,
    TerminalBackend, TerminalFont, TerminalTheme, TerminalView,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
    sync::mpsc::{self, Receiver, Sender},
    time::{SystemTime, UNIX_EPOCH},
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

    result.trim_end_matches(&[' ', '\n'][..]).to_owned()
}

fn synchronized_event_bytes(event: &egui::Event) -> Option<Vec<u8>> {
    match event {
        egui::Event::Text(text) if !terminal_ux::is_multiline_paste(text) => {
            keyboard::committed_text_bytes(text)
        }
        egui::Event::Paste(text)
            if terminal_ux::classify_paste(PastePolicy::ConfirmMultiline, text)
                == PasteDecision::Send =>
        {
            Some(text.as_bytes().to_vec())
        }
        egui::Event::Key {
            key,
            pressed: true,
            modifiers,
            ..
        } if !modifiers.command && !modifiers.ctrl => match key {
            egui::Key::Enter => Some(vec![b'\r']),
            egui::Key::Tab => Some(vec![b'\t']),
            egui::Key::Backspace => Some(vec![0x7f]),
            egui::Key::ArrowUp => Some(b"\x1b[A".to_vec()),
            egui::Key::ArrowDown => Some(b"\x1b[B".to_vec()),
            egui::Key::ArrowRight => Some(b"\x1b[C".to_vec()),
            egui::Key::ArrowLeft => Some(b"\x1b[D".to_vec()),
            egui::Key::Home => Some(b"\x1b[H".to_vec()),
            egui::Key::End => Some(b"\x1b[F".to_vec()),
            egui::Key::Delete => Some(b"\x1b[3~".to_vec()),
            _ => None,
        },
        _ => None,
    }
}

fn terminal_theme(appearance: &TerminalAppearance) -> TerminalTheme {
    let mut palette = ColorPalette::default();
    match appearance.palette {
        TerminalPalette::DefaultDark => {}
        TerminalPalette::Light => {
            palette.foreground = "#202124".into();
            palette.background = "#f7f7f7".into();
            palette.black = "#202124".into();
            palette.red = "#b3261e".into();
            palette.green = "#2e7d32".into();
            palette.yellow = "#8a6d00".into();
            palette.blue = "#1565c0".into();
            palette.magenta = "#8e24aa".into();
            palette.cyan = "#00796b".into();
            palette.white = "#eceff1".into();
            palette.bright_black = "#5f6368".into();
            palette.bright_red = "#d93025".into();
            palette.bright_green = "#188038".into();
            palette.bright_yellow = "#a86f00".into();
            palette.bright_blue = "#1a73e8".into();
            palette.bright_magenta = "#a142f4".into();
            palette.bright_cyan = "#00897b".into();
            palette.bright_white = "#ffffff".into();
            palette.dim_foreground = "#5f6368".into();
        }
        TerminalPalette::HighContrast => {
            palette.foreground = "#ffffff".into();
            palette.background = "#000000".into();
            palette.black = "#000000".into();
            palette.red = "#ff5555".into();
            palette.green = "#55ff55".into();
            palette.yellow = "#ffff55".into();
            palette.blue = "#5555ff".into();
            palette.magenta = "#ff55ff".into();
            palette.cyan = "#55ffff".into();
            palette.white = "#ffffff".into();
            palette.bright_black = "#808080".into();
            palette.bright_red = "#ff8080".into();
            palette.bright_green = "#80ff80".into();
            palette.bright_yellow = "#ffff80".into();
            palette.bright_blue = "#8080ff".into();
            palette.bright_magenta = "#ff80ff".into();
            palette.bright_cyan = "#80ffff".into();
            palette.bright_white = "#ffffff".into();
            palette.dim_foreground = "#b0b0b0".into();
        }
    }
    if let Some(foreground) = &appearance.foreground {
        palette.foreground = foreground.clone();
    }
    if let Some(background) = &appearance.background {
        palette.background = background.clone();
    }
    TerminalTheme::new(Box::new(palette))
}

fn terminal_font(appearance: &TerminalAppearance) -> TerminalFont {
    let font_type = match appearance.font_family {
        TerminalFontFamily::Monospace => egui::FontId::monospace(appearance.font_size),
        TerminalFontFamily::Proportional => egui::FontId::proportional(appearance.font_size),
    };
    TerminalFont::new(FontSettings { font_type })
}

fn terminal_cursor_style(style: TerminalCursorStyle) -> CursorStyle {
    match style {
        TerminalCursorStyle::Block => CursorStyle::Block,
        TerminalCursorStyle::Underline => CursorStyle::Underline,
        TerminalCursorStyle::Beam => CursorStyle::Beam,
    }
}

fn terminal_interaction(appearance: &TerminalAppearance) -> InteractionSettings {
    InteractionSettings {
        select_to_copy: appearance.select_to_copy,
        middle_click_paste: appearance.middle_click_paste,
        right_click_paste: appearance.right_click_paste,
        hide_pointer_while_typing: appearance.hide_pointer_while_typing,
    }
}

fn appearance_controls(
    ui: &mut egui::Ui,
    appearance: &mut TerminalAppearance,
    id_source: &str,
    show_opacity: bool,
) {
    ui.horizontal_wrapped(|ui| {
        ui.label("Font");
        egui::ComboBox::from_id_salt(format!("{id_source}_terminal_font_family"))
            .selected_text(match appearance.font_family {
                TerminalFontFamily::Monospace => "Monospace",
                TerminalFontFamily::Proportional => "Proportional",
            })
            .show_ui(ui, |ui| {
                ui.selectable_value(
                    &mut appearance.font_family,
                    TerminalFontFamily::Monospace,
                    "Monospace",
                );
                ui.selectable_value(
                    &mut appearance.font_family,
                    TerminalFontFamily::Proportional,
                    "Proportional",
                );
            });
        ui.add(egui::Slider::new(&mut appearance.font_size, 8.0..=36.0).text("size"));

        ui.label("Theme");
        egui::ComboBox::from_id_salt(format!("{id_source}_terminal_palette"))
            .selected_text(match appearance.palette {
                TerminalPalette::DefaultDark => "Default dark",
                TerminalPalette::Light => "Light",
                TerminalPalette::HighContrast => "High contrast",
            })
            .show_ui(ui, |ui| {
                ui.selectable_value(
                    &mut appearance.palette,
                    TerminalPalette::DefaultDark,
                    "Default dark",
                );
                ui.selectable_value(&mut appearance.palette, TerminalPalette::Light, "Light");
                ui.selectable_value(
                    &mut appearance.palette,
                    TerminalPalette::HighContrast,
                    "High contrast",
                );
            });

        ui.label("Cursor");
        egui::ComboBox::from_id_salt(format!("{id_source}_terminal_cursor_style"))
            .selected_text(match appearance.cursor_style {
                TerminalCursorStyle::Block => "Block",
                TerminalCursorStyle::Underline => "Underline",
                TerminalCursorStyle::Beam => "Beam",
            })
            .show_ui(ui, |ui| {
                ui.selectable_value(
                    &mut appearance.cursor_style,
                    TerminalCursorStyle::Block,
                    "Block",
                );
                ui.selectable_value(
                    &mut appearance.cursor_style,
                    TerminalCursorStyle::Underline,
                    "Underline",
                );
                ui.selectable_value(
                    &mut appearance.cursor_style,
                    TerminalCursorStyle::Beam,
                    "Beam",
                );
            });
    });

    ui.horizontal_wrapped(|ui| {
        let mut custom_foreground = appearance.foreground.is_some();
        if ui
            .checkbox(&mut custom_foreground, "Custom foreground")
            .changed()
        {
            appearance.foreground =
                custom_foreground.then(|| appearance.palette_defaults().0.into());
        }
        if let Some(value) = appearance.foreground.as_mut() {
            ui.add(egui::TextEdit::singleline(value).desired_width(90.0));
        }

        let mut custom_background = appearance.background.is_some();
        if ui
            .checkbox(&mut custom_background, "Custom background")
            .changed()
        {
            appearance.background =
                custom_background.then(|| appearance.palette_defaults().1.into());
        }
        if let Some(value) = appearance.background.as_mut() {
            ui.add(egui::TextEdit::singleline(value).desired_width(90.0));
        }
    });

    ui.horizontal_wrapped(|ui| {
        ui.checkbox(&mut appearance.select_to_copy, "Select to copy");
        ui.checkbox(&mut appearance.middle_click_paste, "Middle-click paste");
        ui.checkbox(&mut appearance.right_click_paste, "Right-click paste");
        ui.checkbox(
            &mut appearance.hide_pointer_while_typing,
            "Hide pointer while typing",
        );
    });

    if show_opacity {
        ui.add(egui::Slider::new(&mut appearance.opacity, 0.35..=1.0).text("window opacity"));
        ui.small(
            "Window opacity is saved, but the current native eframe window stack does not expose portable runtime opacity; it is not applied on this build.",
        );
    } else {
        ui.small("Window opacity is a global-only preference.");
    }
    if let Some(warning) = appearance.contrast_warning() {
        ui.colored_label(egui::Color32::YELLOW, warning);
    }
}

fn full_profile_override(appearance: &TerminalAppearance) -> AppearanceOverride {
    AppearanceOverride {
        font_family: Some(appearance.font_family),
        font_size: Some(appearance.font_size),
        palette: Some(appearance.palette),
        foreground: appearance.foreground.clone(),
        background: appearance.background.clone(),
        cursor_style: Some(appearance.cursor_style),
        select_to_copy: Some(appearance.select_to_copy),
        middle_click_paste: Some(appearance.middle_click_paste),
        right_click_paste: Some(appearance.right_click_paste),
        hide_pointer_while_typing: Some(appearance.hide_pointer_while_typing),
    }
}

fn render_terminal_tab(
    ui: &mut egui::Ui,
    ctx: &egui::Context,
    tab: &mut Tab,
    terminal_focus: Option<u64>,
    copy_selected: bool,
    show_pane_header: bool,
    appearance: &TerminalAppearance,
) -> (Option<(u64, TabKind, Session)>, bool, bool) {
    let mut reconnect = None;
    let mut focus = false;
    let mut close = false;

    if show_pane_header {
        ui.horizontal(|ui| {
            ui.strong(&tab.name);
            if terminal_focus == Some(tab.id) && !tab.exited {
                ui.strong("● FOCUSED");
            }
            if ui.small_button("Close pane").clicked() {
                close = true;
            }
        });
    }
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
        .set_theme(terminal_theme(appearance))
        .set_font(terminal_font(appearance))
        .set_cursor_style(terminal_cursor_style(appearance.cursor_style))
        .set_interaction(terminal_interaction(appearance))
        .set_focus(terminal_accepts_keyboard(
            terminal_focus,
            tab.id,
            tab.exited,
        ));
    let response = ui.add(view);
    if copy_selected {
        let selected = tab.terminal.selectable_content();
        if !selected.is_empty() {
            ctx.copy_text(selected);
        }
    }
    if response.clicked() && !tab.exited {
        focus = true;
    }
    (reconnect, focus, close)
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
    startup_path: PathBuf,
    startup_settings: StartupSettings,
    startup_pending: bool,
    startup_workspace_path: String,
    startup_notice: String,
    appearance_path: PathBuf,
    appearance_settings: AppearanceSettings,
    appearance_notice: String,
    draft: Session,
    profile_tags: String,
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
    tab_selected: BTreeSet<u64>,
    tab_labels: BTreeMap<u64, TabVisualLabel>,
    tab_switcher_open: bool,
    tab_switcher_query: String,
    tab_switcher_index: usize,
    bulk_close_confirm: Vec<u64>,
    paste_policy: PastePolicy,
    pending_paste: Option<(u64, String)>,
    paste_notice: String,
    search_open: bool,
    search_query: String,
    history_states: BTreeMap<u64, HistoryState>,
    history_timestamps: bool,
    history_notice: String,
    logging_tab: Option<u64>,
    session_log_path: String,
    last_logged_screen: String,
    log_notice: String,
    workspace_panes: Vec<u64>,
    workspace_axis: SplitAxis,
    workspace_path: String,
    workspace_reconnect_on_restore: bool,
    workspace_loaded: Option<WorkspaceLayout>,
    workspace_notice: String,
    sync_input: SyncInputState,
    next_id: u64,
    tx: Sender<(u64, PtyEvent)>,
    rx: Receiver<(u64, PtyEvent)>,
}

impl App {
    pub fn new(path: PathBuf, config: Option<PathBuf>) -> Self {
        let startup_path = startup::settings_path(&path);
        let (startup_settings, startup_error) = match startup::load_settings(&startup_path) {
            Ok(settings) => (settings, String::new()),
            Err(error) => (
                StartupSettings::default(),
                format!("Startup settings ignored: {error:#}"),
            ),
        };
        let appearance_path = appearance::settings_path(&path);
        let (appearance_settings, appearance_error) =
            match appearance::load_settings(&appearance_path) {
                Ok(settings) => (settings, String::new()),
                Err(error) => (
                    AppearanceSettings::default(),
                    format!("Appearance settings ignored: {error:#}"),
                ),
            };
        let (profiles, mut error, writable) = match load_sessions(&path) {
            Ok(profiles) => (profiles, String::new(), true),
            Err(error) => (
                Vec::new(),
                format!("{error:#}. Saving disabled: repair the profile file and restart."),
                false,
            ),
        };
        let (tx, rx) = mpsc::channel();
        for settings_error in [&startup_error, &appearance_error] {
            if !settings_error.is_empty() {
                if !error.is_empty() {
                    error.push('\n');
                }
                error.push_str(settings_error);
            }
        }

        let startup_workspace_path = match &startup_settings.behavior {
            StartupBehavior::Workspace { path } => path.clone(),
            _ => String::new(),
        };

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
            startup_path,
            startup_settings,
            startup_pending: true,
            startup_workspace_path,
            startup_notice: String::new(),
            appearance_path,
            appearance_settings,
            appearance_notice: String::new(),
            draft: Session::default(),
            profile_tags: String::new(),
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
            tab_selected: BTreeSet::new(),
            tab_labels: BTreeMap::new(),
            tab_switcher_open: false,
            tab_switcher_query: String::new(),
            tab_switcher_index: 0,
            bulk_close_confirm: Vec::new(),
            paste_policy: PastePolicy::default(),
            pending_paste: None,
            paste_notice: String::new(),
            search_open: false,
            search_query: String::new(),
            history_states: BTreeMap::new(),
            history_timestamps: false,
            history_notice: String::new(),
            logging_tab: None,
            session_log_path: String::new(),
            last_logged_screen: String::new(),
            log_notice: String::new(),
            workspace_panes: Vec::new(),
            workspace_axis: SplitAxis::Horizontal,
            workspace_path: String::new(),
            workspace_reconnect_on_restore: false,
            workspace_loaded: None,
            workspace_notice: String::new(),
            sync_input: SyncInputState::default(),
            next_id: 1,
            tx,
            rx,
        }
    }

    pub fn storage_writable(&self) -> bool {
        self.writable
    }

    fn load_draft(&mut self, session: Session) {
        self.profile_tags = session.tags.join(", ");
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
        session.tags = self
            .profile_tags
            .split(',')
            .map(str::trim)
            .filter(|tag| !tag.is_empty())
            .map(ToOwned::to_owned)
            .collect();
        session.ssh.dynamic_forwards = parse_forward_lines(&self.dynamic_forwards);
        session.ssh_args()?;
        Ok(session)
    }

    fn tab_order(&self) -> Vec<u64> {
        self.tabs.iter().map(|tab| tab.id).collect()
    }

    fn apply_tab_order(&mut self, order: &[u64]) {
        self.tabs.sort_by_key(|tab| {
            order
                .iter()
                .position(|id| *id == tab.id)
                .unwrap_or(usize::MAX)
        });
    }

    fn confirmation_open(&self) -> bool {
        self.pending_paste.is_some()
            || self.delete_confirm.is_some()
            || self.replace_import_confirm.is_some()
            || self.host_key_remove_confirm.is_some()
            || !self.bulk_close_confirm.is_empty()
    }

    fn connect_draft(&mut self, ctx: &egui::Context) {
        if self.tabs.len() >= 16 {
            self.error = "At most 16 terminal tabs may be open.".into();
            return;
        }
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
        self.error = result.err().map(|error| format!("{error:#}")).unwrap_or_default();
    }

    fn shortcut_action(&self, ctx: &egui::Context) -> Option<ShortcutAction> {
        let confirmation_open = self.confirmation_open();
        ctx.input(|input| {
            let modifiers = ShortcutModifiers {
                command: input.modifiers.command,
                shift: input.modifiers.shift,
                alt: input.modifiers.alt,
            };
            [
                (egui::Key::Enter, ShortcutKey::Enter),
                (egui::Key::K, ShortcutKey::K),
                (egui::Key::F, ShortcutKey::F),
                (egui::Key::W, ShortcutKey::W),
                (egui::Key::ArrowLeft, ShortcutKey::ArrowLeft),
                (egui::Key::ArrowRight, ShortcutKey::ArrowRight),
            ]
            .into_iter()
            .find_map(|(egui_key, shortcut_key)| {
                input
                    .key_pressed(egui_key)
                    .then(|| keyboard::action_for(shortcut_key, modifiers, confirmation_open))
                    .flatten()
            })
        })
    }

    fn close_tab_ids(&mut self, ids: &[u64]) {
        if ids.is_empty() {
            return;
        }
        let order = self.tab_order();
        let next_active = tab_management::next_active_after_close(&order, self.active, ids);
        let closing: BTreeSet<u64> = ids.iter().copied().collect();

        for id in ids {
            self.workspace_panes.retain(|pane| pane != id);
            self.sync_input.remove_pane(*id);
            self.tab_selected.remove(id);
            self.tab_labels.remove(id);
            self.history_states.remove(id);
            if self.logging_tab == Some(*id) {
                self.logging_tab = None;
            }
            if self
                .pending_paste
                .as_ref()
                .is_some_and(|(owner, _)| owner == id)
            {
                self.pending_paste = None;
            }
        }

        // This is the sole ownership-removal point: each TerminalBackend is dropped once.
        tab_management::remove_items_by_id(&mut self.tabs, &closing, |tab| tab.id);
        self.active = next_active;
        if self.terminal_focus.is_some_and(|id| closing.contains(&id)) {
            self.terminal_focus = self.active;
        }
        self.bulk_close_confirm.clear();
    }

    pub fn ui(&mut self, ctx: &egui::Context) {
        if self.startup_pending {
            self.startup_pending = false;
            match self.startup_settings.behavior.clone() {
                StartupBehavior::None => {}
                StartupBehavior::Profile { profile } => {
                    let result = (|| -> anyhow::Result<()> {
                        let session = self
                            .profiles
                            .iter()
                            .find(|session| session_profile_key(session) == profile)
                            .cloned()
                            .ok_or_else(|| {
                                anyhow::anyhow!("configured startup profile no longer exists")
                            })?;
                        anyhow::ensure!(self.tabs.len() < 16, "tab limit reached");
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
                    })();
                    self.startup_notice = match result {
                        Ok(()) => "Opened configured startup profile.".into(),
                        Err(error) => format!("Startup profile not opened: {error:#}"),
                    };
                }
                StartupBehavior::Workspace { path } => {
                    match workspace::load_layout(&PathBuf::from(&path)) {
                        Ok(layout) => {
                            self.workspace_path = path;
                            self.workspace_axis = layout.axis;
                            self.workspace_reconnect_on_restore = layout.reconnect_on_restore;
                            self.workspace_loaded = Some(layout);
                            self.startup_notice =
                                "Loaded startup workspace metadata. Reconnect still requires the explicit Restore & reconnect action.".into();
                        }
                        Err(error) => {
                            self.startup_notice =
                                format!("Startup workspace metadata not loaded: {error:#}");
                        }
                    }
                }
            }
        }

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

        if let Some(source) = active_terminal.filter(|id| self.terminal_focus == Some(*id))
            && self.sync_input.armed()
        {
            let destinations = self.sync_input.destinations(source);
            let payloads: Vec<Vec<u8>> = ctx.input(|input| {
                input
                    .events
                    .iter()
                    .filter_map(synchronized_event_bytes)
                    .collect()
            });
            for destination in destinations {
                if let Some(tab) = self
                    .tabs
                    .iter_mut()
                    .find(|tab| tab.id == destination && !tab.exited)
                {
                    for payload in &payloads {
                        tab.terminal
                            .process_command(BackendCommand::Write(payload.clone()));
                    }
                }
            }
        }
        if let Some(action) = self.shortcut_action(ctx) {
            match action {
                ShortcutAction::Connect => {
                    // Ctrl/Cmd+Enter is a form shortcut only. Never steal it from a focused PTY.
                    if self.terminal_focus.is_none() {
                        self.connect_draft(ctx);
                    }
                }
                ShortcutAction::QuickSwitch => {
                    self.tab_switcher_open = true;
                    self.tab_switcher_query.clear();
                    self.tab_switcher_index = 0;
                    self.terminal_focus = None;
                }
                ShortcutAction::Search => {
                    self.search_open = true;
                    self.terminal_focus = None;
                }
                ShortcutAction::CloseActive => {
                    if let Some(id) = self.active {
                        self.close_tab_ids(&[id]);
                    }
                }
                ShortcutAction::FocusPreviousPane | ShortcutAction::FocusNextPane => {
                    let direction = if action == ShortcutAction::FocusPreviousPane {
                        -1
                    } else {
                        1
                    };
                    if let Some(id) =
                        keyboard::adjacent_pane(&self.workspace_panes, self.terminal_focus, direction)
                    {
                        self.active = Some(id);
                        self.terminal_focus = Some(id);
                    }
                }
            }
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
                    egui::CollapsingHeader::new("Keyboard & accessibility")
                        .default_open(false)
                        .show(ui, |ui| {
                            ui.label("Connect: Ctrl/Cmd+Enter");
                            ui.label("Quick switch tabs: Ctrl/Cmd+Shift+K");
                            ui.label("Search history: Ctrl/Cmd+Shift+F");
                            ui.label("Close active tab: Ctrl/Cmd+W");
                            ui.label("Focus split pane: Ctrl/Cmd+Alt+Left/Right");
                            ui.small(
                                "Application shortcuts are disabled while a confirmation dialog is open. Tab/Shift+Tab traverses ordinary egui controls.",
                            );
                            ui.small(
                                "IME pre-edit and screen-reader integration depend on egui/winit and the native platform; committed Unicode text is forwarded unchanged.",
                            );
                        });
                    ui.separator();
                    ui.label("Saved sessions");
                    ui.horizontal(|ui| {
                        if ui
                            .add(
                                egui::TextEdit::singleline(&mut self.profile_query)
                                    .hint_text("Search name, folder, tags, host or user"),
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
                            let mut visible: Vec<&Session> = self
                                .profiles
                                .iter()
                                .filter(|profile| session_matches_query(profile, &self.profile_query))
                                .collect();
                            visible.sort_by(|left, right| {
                                right
                                    .favorite
                                    .cmp(&left.favorite)
                                    .then_with(|| left.folder.cmp(&right.folder))
                                    .then_with(|| left.name.cmp(&right.name))
                            });
                            for profile in visible {
                                let key = session_profile_key(profile);
                                let is_selected =
                                    self.selected_profile.as_deref() == Some(key.as_str());
                                let location = if profile.folder.is_empty() {
                                    profile.name.clone()
                                } else {
                                    format!("{}/{}", profile.folder, profile.name)
                                };
                                let tags = if profile.tags.is_empty() {
                                    String::new()
                                } else {
                                    format!("  [{}]", profile.tags.join(", "))
                                };
                                let label = format!(
                                    "{}{}{}",
                                    if profile.favorite { "★ " } else { "" },
                                    location,
                                    tags
                                );
                                if ui.selectable_label(is_selected, label).clicked() {
                                    selected = Some(profile.clone());
                                }
                            }
                        });
                    if let Some(profile) = selected {
                        self.terminal_focus = None;
                        self.selected_profile = Some(session_profile_key(&profile));
                        self.delete_confirm = None;
                        self.load_draft(profile);
                    }

                    if let Some(selected_key) = self.selected_profile.clone() {
                        ui.horizontal(|ui| {
                            if ui.small_button("Duplicate").clicked()
                                && let Some(source) = self
                                    .profiles
                                    .iter()
                                    .find(|profile| session_profile_key(profile) == selected_key)
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
                                self.delete_confirm = Some(selected_key.clone());
                            }
                        });
                    }

                    if let Some(name) = self.delete_confirm.clone() {
                        ui.group(|ui| {
                            let display_name = self
                                .profiles
                                .iter()
                                .find(|profile| session_profile_key(profile) == name)
                                .map(|profile| {
                                    if profile.folder.is_empty() {
                                        profile.name.clone()
                                    } else {
                                        format!("{}/{}", profile.folder, profile.name)
                                    }
                                })
                                .unwrap_or_else(|| name.clone());
                            ui.label(format!("Delete saved profile {display_name:?}?"));
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

                    egui::CollapsingHeader::new("Startup behavior")
                        .default_open(false)
                        .show(ui, |ui| {
                            ui.small(
                                "Startup choices are stored separately from profile imports. A workspace choice loads metadata only; reconnect remains an explicit action.",
                            );
                            if ui.button("Start with no session").clicked() {
                                let settings = StartupSettings {
                                    behavior: StartupBehavior::None,
                                };
                                match startup::save_settings(&self.startup_path, &settings) {
                                    Ok(()) => {
                                        self.startup_settings = settings;
                                        self.startup_notice = "Startup action disabled.".into();
                                    }
                                    Err(error) => {
                                        self.startup_notice =
                                            format!("Cannot save startup settings: {error:#}");
                                    }
                                }
                            }
                            let can_use_selected = self.selected_profile.is_some();
                            if ui
                                .add_enabled(
                                    can_use_selected,
                                    egui::Button::new("Open selected profile on startup"),
                                )
                                .clicked()
                                && let Some(profile) = self.selected_profile.clone()
                            {
                                let settings = StartupSettings {
                                    behavior: StartupBehavior::Profile { profile },
                                };
                                match startup::save_settings(&self.startup_path, &settings) {
                                    Ok(()) => {
                                        self.startup_settings = settings;
                                        self.startup_notice =
                                            "Selected profile will open on next startup using normal host-key and authentication policy.".into();
                                    }
                                    Err(error) => {
                                        self.startup_notice =
                                            format!("Cannot save startup settings: {error:#}");
                                    }
                                }
                            }
                            ui.label("Workspace layout path");
                            if ui
                                .text_edit_singleline(&mut self.startup_workspace_path)
                                .has_focus()
                            {
                                self.terminal_focus = None;
                            }
                            if ui.button("Load workspace metadata on startup").clicked() {
                                let settings = StartupSettings {
                                    behavior: StartupBehavior::Workspace {
                                        path: self.startup_workspace_path.trim().to_owned(),
                                    },
                                };
                                match startup::save_settings(&self.startup_path, &settings) {
                                    Ok(()) => {
                                        self.startup_settings = settings;
                                        self.startup_notice =
                                            "Workspace metadata will load on next startup; it will not reconnect automatically.".into();
                                    }
                                    Err(error) => {
                                        self.startup_notice =
                                            format!("Cannot save startup settings: {error:#}");
                                    }
                                }
                            }
                            if !self.startup_notice.is_empty() {
                                ui.small(&self.startup_notice);
                            }
                        });

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

                    ui.separator();
                    ui.strong("Profile organization");
                    ui.label("Folder");
                    if ui.text_edit_singleline(&mut self.draft.folder).has_focus() {
                        self.terminal_focus = None;
                    }
                    ui.horizontal(|ui| {
                        ui.checkbox(&mut self.draft.favorite, "Favorite / pin");
                    });
                    ui.label("Tags (comma separated)");
                    if ui.text_edit_singleline(&mut self.profile_tags).has_focus() {
                        self.terminal_focus = None;
                    }
                    ui.small("Folders and tags are non-secret metadata. Moving or renaming a saved profile never disconnects already-open tabs.");

                    ui.horizontal(|ui| {
                        if ui
                            .add_enabled(self.writable, egui::Button::new("Save profile"))
                            .clicked()
                        {
                            self.terminal_focus = None;
                            let selected_name = self.selected_profile.clone();
                            let result = self.validated_draft().and_then(|session| {
                                let saved_key = session_profile_key(&session);
                                let next = save_session_edit(
                                    &self.profiles,
                                    selected_name.as_deref(),
                                    session,
                                )?;
                                save_sessions(&self.path, &next)?;
                                self.profiles = next;
                                self.selected_profile = Some(saved_key);
                                self.delete_confirm = None;
                                Ok(())
                            });
                            self.error =
                                result.err().map(|e| format!("{e:#}")).unwrap_or_default();
                        }
                        if ui
                            .add_enabled(self.tabs.len() < 16, egui::Button::new("Connect"))
                            .on_hover_text("Keyboard: Ctrl/Cmd+Enter")
                            .clicked()
                        {
                            self.connect_draft(ctx);
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

        let tab_snapshot: Vec<(u64, String, bool)> = self
            .tabs
            .iter()
            .map(|tab| (tab.id, tab.name.clone(), tab.exited))
            .collect();
        let mut close_one = None;
        let mut move_active = None;
        let mut bulk_mode = None;

        egui::TopBottomPanel::top("tabs").show(ctx, |ui| {
            ui.horizontal_wrapped(|ui| {
                if ui
                    .button("Quick switch…")
                    .on_hover_text("Ctrl/Cmd+Shift+K")
                    .clicked()
                {
                    self.tab_switcher_open = true;
                    self.tab_switcher_query.clear();
                    self.tab_switcher_index = 0;
                    self.terminal_focus = None;
                }
                if ui
                    .add_enabled(self.active.is_some(), egui::Button::new("← Move"))
                    .clicked()
                {
                    move_active = Some(-1);
                }
                if ui
                    .add_enabled(self.active.is_some(), egui::Button::new("Move →"))
                    .clicked()
                {
                    move_active = Some(1);
                }
                if ui
                    .add_enabled(
                        !self.tab_selected.is_empty(),
                        egui::Button::new("Close selected"),
                    )
                    .clicked()
                {
                    bulk_mode = Some(BulkCloseMode::Selected);
                }
                if let Some(active) = self.active {
                    if ui.button("Close right").clicked() {
                        bulk_mode = Some(BulkCloseMode::RightOf(active));
                    }
                    if ui.button("Close others").clicked() {
                        bulk_mode = Some(BulkCloseMode::Others(active));
                    }
                    let mut visual = self.tab_labels.get(&active).copied().unwrap_or_default();
                    egui::ComboBox::from_id_salt("active_tab_visual_label")
                        .selected_text(format!("Label: {}", visual.name()))
                        .show_ui(ui, |ui| {
                            for choice in TabVisualLabel::ALL {
                                ui.selectable_value(&mut visual, choice, choice.name());
                            }
                        });
                    if visual == TabVisualLabel::None {
                        self.tab_labels.remove(&active);
                    } else {
                        self.tab_labels.insert(active, visual);
                    }
                }
            });
            ui.horizontal_wrapped(|ui| {
                for (id, name, exited) in &tab_snapshot {
                    let mut selected = self.tab_selected.contains(id);
                    if ui
                        .checkbox(&mut selected, "")
                        .on_hover_text("Select for bulk tab actions")
                        .changed()
                    {
                        if selected {
                            self.tab_selected.insert(*id);
                        } else {
                            self.tab_selected.remove(id);
                        }
                    }
                    let marker = self
                        .tab_labels
                        .get(id)
                        .copied()
                        .unwrap_or_default()
                        .marker();
                    let label = format!(
                        "{}{}{}{}",
                        marker,
                        if self.terminal_focus == Some(*id) {
                            "● "
                        } else {
                            ""
                        },
                        name,
                        if *exited { " (exited)" } else { "" }
                    );
                    if ui
                        .selectable_label(self.active == Some(*id), label)
                        .clicked()
                    {
                        self.active = Some(*id);
                        self.terminal_focus = Some(*id);
                    }
                    if ui
                        .small_button("×")
                        .on_hover_text("Disconnect and close terminal")
                        .clicked()
                    {
                        close_one = Some(*id);
                    }
                }
            });
        });

        if let Some(delta) = move_active
            && let Some(active) = self.active
        {
            let order = self.tab_order();
            let next = tab_management::moved_order(&order, active, delta);
            self.apply_tab_order(&next);
        }
        if let Some(mode) = bulk_mode {
            let ids = tab_management::bulk_close_ids(&self.tab_order(), &self.tab_selected, mode);
            if !ids.is_empty() {
                self.bulk_close_confirm = ids;
                self.terminal_focus = None;
            }
        }
        if let Some(id) = close_one {
            self.close_tab_ids(&[id]);
        }

        if !self.bulk_close_confirm.is_empty() {
            let ids = self.bulk_close_confirm.clone();
            let names: Vec<String> = ids
                .iter()
                .filter_map(|id| {
                    self.tabs.iter().find(|tab| tab.id == *id).map(|tab| {
                        format!(
                            "{}{}",
                            tab.name,
                            if tab.exited { " (exited)" } else { " (live)" }
                        )
                    })
                })
                .collect();
            let live_count = ids
                .iter()
                .filter(|id| self.tabs.iter().any(|tab| tab.id == **id && !tab.exited))
                .count();
            let mut confirm = false;
            let mut cancel = false;
            egui::Window::new("Confirm bulk tab close")
                .collapsible(false)
                .resizable(false)
                .show(ctx, |ui| {
                    ui.strong(format!(
                        "Close {} tab(s), including {} live session(s)?",
                        ids.len(),
                        live_count
                    ));
                    for name in &names {
                        ui.label(format!("• {name}"));
                    }
                    ui.small("Closing drops each owned terminal/PTy once and disarms synchronized input if its target set changes.");
                    ui.horizontal(|ui| {
                        if ui.button("Close listed tabs").clicked() {
                            confirm = true;
                        }
                        if ui.button("Cancel").clicked() {
                            cancel = true;
                        }
                    });
                });
            if confirm {
                self.close_tab_ids(&ids);
            } else if cancel {
                self.bulk_close_confirm.clear();
                self.terminal_focus = self.active;
            }
        }

        if self.tab_switcher_open {
            let matches = tab_management::matching_tab_ids(
                self.tabs.iter().map(|tab| (tab.id, tab.name.as_str())),
                &self.tab_switcher_query,
            );
            if matches.is_empty() {
                self.tab_switcher_index = 0;
            } else {
                self.tab_switcher_index = self.tab_switcher_index.min(matches.len() - 1);
            }
            if ctx.input(|input| input.key_pressed(egui::Key::ArrowDown)) && !matches.is_empty() {
                self.tab_switcher_index = (self.tab_switcher_index + 1) % matches.len();
            }
            if ctx.input(|input| input.key_pressed(egui::Key::ArrowUp)) && !matches.is_empty() {
                self.tab_switcher_index =
                    (self.tab_switcher_index + matches.len() - 1) % matches.len();
            }
            let activate = ctx.input(|input| input.key_pressed(egui::Key::Enter));
            let escape = ctx.input(|input| input.key_pressed(egui::Key::Escape));
            let mut clicked = None;
            egui::Window::new("Quick tab switcher")
                .collapsible(false)
                .resizable(false)
                .show(ctx, |ui| {
                    let response = ui.add(
                        egui::TextEdit::singleline(&mut self.tab_switcher_query)
                            .hint_text("Search open tabs"),
                    );
                    response.request_focus();
                    ui.small("Keyboard: ↑/↓ choose, Enter switch, Esc close");
                    for (index, id) in matches.iter().enumerate() {
                        if let Some(tab) = self.tabs.iter().find(|tab| tab.id == *id)
                            && ui
                                .selectable_label(index == self.tab_switcher_index, &tab.name)
                                .clicked()
                        {
                            clicked = Some(*id);
                        }
                    }
                });
            let chosen = clicked.or_else(|| {
                (activate && !matches.is_empty()).then(|| matches[self.tab_switcher_index])
            });
            if let Some(id) = chosen {
                self.active = Some(id);
                self.terminal_focus = Some(id);
                self.tab_switcher_open = false;
            } else if escape {
                self.tab_switcher_open = false;
                self.terminal_focus = self.active;
            }
        }

        let mut reconnect = None;
        let mut close_browser = false;
        let mut close_scp = false;
        let mut close_pane = None;
        let mut focus_pane = None;
        let mut split_requested = None;
        let mut workspace_save_requested = false;
        let mut workspace_load_requested = false;
        let mut workspace_restore_requested = false;
        let mut copy_selected = false;
        let mut history_scroll_request = None;

        if self.search_open
            && let Some(id) = self.active
            && let Some(tab) = self.tabs.iter().find(|tab| tab.id == id)
        {
            let snapshot = tab.terminal.history_lines();
            let state = self.history_states.entry(id).or_default();
            state.update_snapshot(snapshot);
            state.set_query(self.search_query.clone());

            if ctx.input(|input| input.key_pressed(egui::Key::F3)) {
                if ctx.input(|input| input.modifiers.shift) {
                    state.select_previous_hit();
                } else {
                    state.select_next_hit();
                }
                if let Some(hit) = state.selected_hit() {
                    history_scroll_request = Some((id, hit.line_number.saturating_sub(1)));
                }
            }
        }

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

                let active_profile_name = self.active.and_then(|id| {
                    self.tabs
                        .iter()
                        .find(|tab| tab.id == id)
                        .map(|tab| tab.session.name.clone())
                });
                egui::CollapsingHeader::new("Appearance & interaction")
                    .default_open(false)
                    .show(ui, |ui| {
                        ui.strong("Global defaults");
                        appearance_controls(
                            ui,
                            &mut self.appearance_settings.global,
                            "global",
                            true,
                        );

                        if let Some(profile_name) = active_profile_name.as_deref() {
                            ui.separator();
                            let effective_before =
                                self.appearance_settings.effective_for(profile_name);
                            let mut override_enabled =
                                self.appearance_settings.profiles.contains_key(profile_name);
                            if ui
                                .checkbox(
                                    &mut override_enabled,
                                    format!("Override global settings for profile {profile_name:?}"),
                                )
                                .changed()
                            {
                                if override_enabled {
                                    self.appearance_settings.profiles.insert(
                                        profile_name.to_owned(),
                                        full_profile_override(&effective_before),
                                    );
                                } else {
                                    self.appearance_settings.profiles.remove(profile_name);
                                }
                            }

                            if override_enabled {
                                let mut effective =
                                    self.appearance_settings.effective_for(profile_name);
                                appearance_controls(
                                    ui,
                                    &mut effective,
                                    "active_profile",
                                    false,
                                );
                                self.appearance_settings.profiles.insert(
                                    profile_name.to_owned(),
                                    full_profile_override(&effective),
                                );
                            }
                        } else {
                            ui.small(
                                "Open a terminal tab to configure an optional profile-level override.",
                            );
                        }

                        ui.horizontal(|ui| {
                            if ui.button("Save appearance settings").clicked() {
                                self.appearance_notice =
                                    match appearance::save_settings(
                                        &self.appearance_path,
                                        &self.appearance_settings,
                                    ) {
                                        Ok(()) => {
                                            "Appearance settings saved atomically.".into()
                                        }
                                        Err(error) => {
                                            format!("Cannot save appearance settings: {error:#}")
                                        }
                                    };
                            }
                            if ui.button("Reset global defaults").clicked() {
                                self.appearance_settings.global = TerminalAppearance::default();
                                self.appearance_notice =
                                    "Global appearance reset in memory; press Save to persist."
                                        .into();
                            }
                        });
                        ui.small(
                            "Profile overrides apply only to that profile. Unset profiles inherit global preferences. Mouse-triggered paste requests still enter the same guarded paste policy shown above.",
                        );
                        if !self.appearance_notice.is_empty() {
                            ui.small(&self.appearance_notice);
                        }
                    });

                if self.search_open {
                    let active_id = self.active;
                    ui.horizontal_wrapped(|ui| {
                        ui.label("Find retained history");
                        let response = ui.add(
                            egui::TextEdit::singleline(&mut self.search_query)
                                .hint_text("Search retained scrollback"),
                        );
                        if response.has_focus() {
                            self.terminal_focus = None;
                        }

                        if let Some(id) = active_id {
                            let state = self.history_states.entry(id).or_default();
                            state.set_query(self.search_query.clone());

                            if ui.button("Previous").on_hover_text("Shift+F3").clicked() {
                                state.select_previous_hit();
                                if let Some(hit) = state.selected_hit() {
                                    history_scroll_request =
                                        Some((id, hit.line_number.saturating_sub(1)));
                                }
                            }
                            if ui.button("Next").on_hover_text("F3").clicked() {
                                state.select_next_hit();
                                if let Some(hit) = state.selected_hit() {
                                    history_scroll_request =
                                        Some((id, hit.line_number.saturating_sub(1)));
                                }
                            }
                            ui.label(format!(
                                "{} match(es) · {} retained line(s) · {} truncated",
                                state.hits().len(),
                                state.lines().len(),
                                state.truncated_lines()
                            ));
                        }
                    });

                    if let Some(id) = active_id {
                        let state = self.history_states.entry(id).or_default();
                        let selected_index = state.selected_hit_index();
                        let hits = state.hits().to_vec();
                        let mut clicked_hit = None;
                        egui::ScrollArea::vertical()
                            .max_height(130.0)
                            .show(ui, |ui| {
                                for (index, hit) in hits.iter().enumerate() {
                                    let label = format!(
                                        "{}:{}  {}",
                                        hit.line_number, hit.column, hit.preview
                                    );
                                    if ui
                                        .selectable_label(index == selected_index, label)
                                        .clicked()
                                    {
                                        clicked_hit = Some(index);
                                    }
                                }
                            });
                        if let Some(index) = clicked_hit {
                            let state = self.history_states.entry(id).or_default();
                            state.select_hit(index);
                            if let Some(hit) = state.selected_hit() {
                                history_scroll_request =
                                    Some((id, hit.line_number.saturating_sub(1)));
                            }
                        }

                        ui.horizontal_wrapped(|ui| {
                            ui.checkbox(
                                &mut self.history_timestamps,
                                "Timestamp new marks",
                            );
                            if ui.button("Mark output boundary").clicked() {
                                let timestamp = self.history_timestamps.then(|| {
                                    SystemTime::now()
                                        .duration_since(UNIX_EPOCH)
                                        .unwrap_or_default()
                                        .as_secs()
                                });
                                let state = self.history_states.entry(id).or_default();
                                self.history_notice = match state.mark_newest_nonempty(timestamp) {
                                    Some(line_id) => format!("Marked retained line {line_id}."),
                                    None => "No retained output is available to mark.".into(),
                                };
                            }
                            ui.small(
                                "Marks and timestamps stay in memory only. Terminal content is written to disk only by explicit session logging.",
                            );
                        });

                        let marks: Vec<_> = self
                            .history_states
                            .get(&id)
                            .map(|state| state.marks().cloned().collect())
                            .unwrap_or_default();
                        let mut toggle_fold = None;
                        let mut remove_mark = None;
                        if !marks.is_empty() {
                            ui.group(|ui| {
                                ui.strong("History marks");
                                for mark in &marks {
                                    let state = self.history_states.get(&id).unwrap();
                                    let preview = state
                                        .line(mark.line_id)
                                        .map(|line| line.text.as_str())
                                        .unwrap_or("<truncated>");
                                    let folded = state.folded_count_after(mark.line_id);
                                    ui.horizontal_wrapped(|ui| {
                                        let timestamp = mark
                                            .timestamp_epoch_seconds
                                            .map(|value| format!(" · unix {value}"))
                                            .unwrap_or_default();
                                        ui.monospace(format!(
                                            "#{}{}  {}",
                                            mark.line_id, timestamp, preview
                                        ));
                                        if ui
                                            .small_button(if mark.collapsed {
                                                format!("Unfold ({folded})")
                                            } else {
                                                "Fold region".into()
                                            })
                                            .clicked()
                                        {
                                            toggle_fold = Some(mark.line_id);
                                        }
                                        if ui.small_button("Remove mark").clicked() {
                                            remove_mark = Some(mark.line_id);
                                        }
                                    });
                                }
                            });
                        }
                        if let Some(line_id) = toggle_fold {
                            self.history_states
                                .entry(id)
                                .or_default()
                                .toggle_fold(line_id);
                        }
                        if let Some(line_id) = remove_mark {
                            self.history_states
                                .entry(id)
                                .or_default()
                                .remove_mark(line_id);
                        }

                        egui::CollapsingHeader::new("History inspector (last 200 rows)")
                            .default_open(false)
                            .show(ui, |ui| {
                                let rows = self
                                    .history_states
                                    .get(&id)
                                    .map(HistoryState::display_rows)
                                    .unwrap_or_default();
                                let start = rows.len().saturating_sub(200);
                                egui::ScrollArea::vertical()
                                    .max_height(180.0)
                                    .show(ui, |ui| {
                                        for row in &rows[start..] {
                                            match row {
                                                HistoryRow::Line(line) => {
                                                    let marked = self
                                                        .history_states
                                                        .get(&id)
                                                        .is_some_and(|state| {
                                                            state
                                                                .marks()
                                                                .any(|mark| mark.line_id == line.id)
                                                        });
                                                    ui.monospace(format!(
                                                        "{}{}",
                                                        if marked { "◆ " } else { "  " },
                                                        line.text
                                                    ));
                                                }
                                                HistoryRow::Folded { hidden_lines, .. } => {
                                                    ui.strong(format!(
                                                        "  … {hidden_lines} folded retained line(s) …"
                                                    ));
                                                }
                                            }
                                        }
                                    });
                            });
                    }

                    if !self.history_notice.is_empty() {
                        ui.small(&self.history_notice);
                    }
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

            ui.group(|ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.strong("SSH workspace");
                    if ui.button("Split horizontal").clicked() {
                        split_requested = Some(SplitAxis::Horizontal);
                    }
                    if ui.button("Split vertical").clicked() {
                        split_requested = Some(SplitAxis::Vertical);
                    }
                    ui.label("Layout");
                    let path = ui.add(
                        egui::TextEdit::singleline(&mut self.workspace_path)
                            .desired_width(220.0)
                            .hint_text("workspace.json"),
                    );
                    if path.has_focus() {
                        self.terminal_focus = None;
                    }
                    if ui.button("Save").clicked() {
                        workspace_save_requested = true;
                    }
                    if ui.button("Load metadata").clicked() {
                        workspace_load_requested = true;
                    }
                    if ui
                        .add_enabled(
                            self.workspace_loaded
                                .as_ref()
                                .is_some_and(|layout| layout.reconnect_on_restore),
                            egui::Button::new("Restore & reconnect"),
                        )
                        .clicked()
                    {
                        workspace_restore_requested = true;
                    }
                });
                ui.checkbox(
                    &mut self.workspace_reconnect_on_restore,
                    "Allow reconnect when this saved layout is explicitly restored",
                );
                ui.small(
                    "Loading layout metadata never reconnects. Restore & reconnect is a separate explicit action and is only enabled for layouts saved with reconnect permission.",
                );

                if self.workspace_panes.len() >= 2 {
                    ui.horizontal_wrapped(|ui| {
                        ui.label("Sync targets:");
                        for pane_id in self.workspace_panes.clone() {
                            let label = self
                                .tabs
                                .iter()
                                .find(|tab| tab.id == pane_id)
                                .map(|tab| tab.name.clone())
                                .unwrap_or_else(|| format!("pane {pane_id}"));
                            let mut selected = self.sync_input.is_target(pane_id);
                            if ui.checkbox(&mut selected, label).changed() {
                                self.sync_input.set_target(pane_id, selected);
                            }
                        }
                        if self.sync_input.armed() {
                            if ui.button("Disarm synchronized input").clicked() {
                                self.sync_input.set_armed(false);
                            }
                        } else if ui
                            .add_enabled(
                                self.sync_input.selected_count() >= 2,
                                egui::Button::new("Arm synchronized input"),
                            )
                            .clicked()
                        {
                            self.sync_input.set_armed(true);
                        }
                    });
                    if self.sync_input.armed() {
                        ui.strong(
                            "SYNC INPUT ARMED — text, safe paste and navigation keys are mirrored to every selected pane.",
                        );
                    } else {
                        ui.small("Synchronized input is disarmed.");
                    }
                }
                if !self.workspace_notice.is_empty() {
                    ui.small(&self.workspace_notice);
                }
            });
            ui.separator();

            let terminal_focus = self.terminal_focus;
            let split_ids: Vec<u64> = self
                .workspace_panes
                .iter()
                .copied()
                .filter(|id| self.tabs.iter().any(|tab| tab.id == *id))
                .take(2)
                .collect();

            if split_ids.len() == 2 {
                let first_index = self.tabs.iter().position(|tab| tab.id == split_ids[0]);
                let second_index = self.tabs.iter().position(|tab| tab.id == split_ids[1]);
                if let (Some(first_index), Some(second_index)) = (first_index, second_index) {
                    let (first, second) = if first_index < second_index {
                        let (left, right) = self.tabs.split_at_mut(second_index);
                        (&mut left[first_index], &mut right[0])
                    } else {
                        let (left, right) = self.tabs.split_at_mut(first_index);
                        (&mut right[0], &mut left[second_index])
                    };
                    let active = self.active;
                    let appearance_settings = self.appearance_settings.clone();
                    let mut render = |ui: &mut egui::Ui, tab: &mut Tab| {
                        let appearance = appearance_settings.effective_for(&tab.session.name);
                        let (next_reconnect, focused, close) = render_terminal_tab(
                            ui,
                            ctx,
                            tab,
                            terminal_focus,
                            copy_selected && active == Some(tab.id),
                            true,
                            &appearance,
                        );
                        if reconnect.is_none() {
                            reconnect = next_reconnect;
                        }
                        if focused {
                            focus_pane = Some(tab.id);
                        }
                        if close {
                            close_pane = Some(tab.id);
                        }
                    };
                    match self.workspace_axis {
                        SplitAxis::Horizontal => {
                            ui.columns(2, |columns| {
                                render(&mut columns[0], first);
                                render(&mut columns[1], second);
                            });
                        }
                        SplitAxis::Vertical => {
                            let pane_height = (ui.available_height() / 2.0 - 4.0).max(80.0);
                            ui.allocate_ui(
                                egui::vec2(ui.available_width(), pane_height),
                                |ui| render(ui, first),
                            );
                            ui.separator();
                            ui.allocate_ui(
                                egui::vec2(ui.available_width(), pane_height),
                                |ui| render(ui, second),
                            );
                        }
                    }
                }
            } else if let Some(tab) = self.tabs.iter_mut().find(|tab| Some(tab.id) == self.active) {
                let appearance = self.appearance_settings.effective_for(&tab.session.name);
                let (next_reconnect, focused, close) = render_terminal_tab(
                    ui,
                    ctx,
                    tab,
                    terminal_focus,
                    copy_selected,
                    false,
                    &appearance,
                );
                reconnect = next_reconnect;
                if focused {
                    focus_pane = Some(tab.id);
                }
                if close {
                    close_pane = Some(tab.id);
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
        if let Some((id, history_index)) = history_scroll_request
            && let Some(tab) = self.tabs.iter_mut().find(|tab| tab.id == id)
        {
            if !tab.terminal.scroll_to_history_index(history_index) {
                self.history_notice =
                    "Selected history result was truncated before navigation.".into();
            }
            ctx.request_repaint();
        }

        if let Some(id) = focus_pane {
            self.active = Some(id);
            self.terminal_focus = Some(id);
        }

        if let Some(id) = close_pane {
            self.close_tab_ids(&[id]);
        }
        if let Some(axis) = split_requested {
            let source = self.active.and_then(|id| {
                self.tabs
                    .iter()
                    .find(|tab| tab.id == id && !tab.exited && matches!(tab.kind, TabKind::Ssh))
                    .map(|tab| (tab.id, tab.session.clone()))
            });
            if let Some((source_id, session)) = source {
                if self.tabs.len() >= 16 {
                    self.workspace_notice = "Cannot split: the 16-tab limit is reached.".into();
                } else {
                    let id = self.next_id;
                    match terminal::connect(
                        id,
                        ctx.clone(),
                        self.tx.clone(),
                        &session,
                        self.config.as_deref(),
                    ) {
                        Ok(terminal) => {
                            self.tabs.push(Tab {
                                id,
                                name: format!("{} · split", session.name),
                                kind: TabKind::Ssh,
                                session,
                                terminal,
                                exited: false,
                            });
                            self.workspace_panes = vec![source_id, id];
                            self.workspace_axis = axis;
                            self.sync_input = SyncInputState::default();
                            self.active = Some(id);
                            self.terminal_focus = Some(id);
                            self.next_id += 1;
                            self.workspace_notice =
                                "Split created. Synchronized input remains disarmed.".into();
                        }
                        Err(error) => {
                            self.workspace_notice = format!("Cannot create split: {error:#}");
                        }
                    }
                }
            } else {
                self.workspace_notice =
                    "Select a connected SSH tab before creating a split.".into();
            }
        }
        if workspace_save_requested {
            let result = (|| -> anyhow::Result<()> {
                let value = self.workspace_path.trim();
                anyhow::ensure!(!value.is_empty(), "workspace layout path is required");
                let ids: Vec<u64> = if self.workspace_panes.is_empty() {
                    self.active.into_iter().collect()
                } else {
                    self.workspace_panes.clone()
                };
                let panes: Vec<Session> = ids
                    .iter()
                    .filter_map(|id| {
                        self.tabs
                            .iter()
                            .find(|tab| tab.id == *id && matches!(tab.kind, TabKind::Ssh))
                            .map(|tab| tab.session.clone())
                    })
                    .collect();
                let layout = WorkspaceLayout {
                    version: 1,
                    axis: self.workspace_axis,
                    reconnect_on_restore: self.workspace_reconnect_on_restore,
                    panes,
                };
                workspace::save_layout(&PathBuf::from(value), &layout)
            })();
            self.workspace_notice = match result {
                Ok(()) => "Workspace layout saved.".into(),
                Err(error) => format!("Cannot save workspace: {error:#}"),
            };
        }

        if workspace_load_requested {
            let value = self.workspace_path.trim();
            if value.is_empty() {
                self.workspace_notice = "Workspace layout path is required.".into();
            } else {
                match workspace::load_layout(&PathBuf::from(value)) {
                    Ok(layout) => {
                        let pane_count = layout.panes.len();
                        let reconnect = layout.reconnect_on_restore;
                        self.workspace_loaded = Some(layout);
                        self.workspace_notice = if reconnect {
                            format!(
                                "Loaded {pane_count} pane(s) as metadata only. Use Restore & reconnect to connect them."
                            )
                        } else {
                            format!(
                                "Loaded {pane_count} pane(s) as metadata only. Reconnect is disabled in this layout."
                            )
                        };
                    }
                    Err(error) => {
                        self.workspace_loaded = None;
                        self.workspace_notice = format!("Cannot load workspace: {error:#}");
                    }
                }
            }
        }
        if workspace_restore_requested {
            let result = (|| -> anyhow::Result<(Vec<Tab>, SplitAxis)> {
                let layout = self
                    .workspace_loaded
                    .clone()
                    .ok_or_else(|| anyhow::anyhow!("load workspace metadata first"))?;
                anyhow::ensure!(
                    layout.may_reconnect(true),
                    "this workspace was not saved with reconnect permission"
                );
                anyhow::ensure!(
                    self.tabs.len() + layout.panes.len() <= 16,
                    "restoring this workspace would exceed the 16-tab limit"
                );
                let axis = layout.axis;
                let mut created = Vec::new();
                for (offset, session) in layout.panes.into_iter().enumerate() {
                    let id = self.next_id + offset as u64;
                    let terminal = terminal::connect(
                        id,
                        ctx.clone(),
                        self.tx.clone(),
                        &session,
                        self.config.as_deref(),
                    )?;
                    created.push(Tab {
                        id,
                        name: format!("{} · restored", session.name),
                        kind: TabKind::Ssh,
                        session,
                        terminal,
                        exited: false,
                    });
                }
                Ok((created, axis))
            })();

            match result {
                Ok((created, axis)) => {
                    let ids: Vec<u64> = created.iter().map(|tab| tab.id).collect();
                    self.next_id += created.len() as u64;
                    self.tabs.extend(created);
                    self.workspace_panes = ids;
                    self.workspace_axis = axis;
                    self.sync_input = SyncInputState::default();
                    self.active = self.workspace_panes.last().copied();
                    self.terminal_focus = self.active;
                    self.workspace_notice =
                        "Workspace restored by explicit action; synchronized input is disarmed."
                            .into();
                }
                Err(error) => {
                    self.workspace_notice = format!("Cannot restore workspace: {error:#}");
                }
            }
        }
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
                        .filter(|&&byte| matches!(byte, b'\r' | b'\n'))
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
                    let mut preview = text.chars().take(4000).collect::<String>();
                    ui.add(
                        egui::TextEdit::multiline(&mut preview)
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
                let bytes = text.into_bytes();
                let source_exists = if let Some(tab) =
                    self.tabs.iter_mut().find(|tab| tab.id == id && !tab.exited)
                {
                    tab.terminal
                        .process_command(BackendCommand::Write(bytes.clone()));
                    true
                } else {
                    false
                };
                if source_exists {
                    for destination in self.sync_input.destinations(id) {
                        if let Some(target) = self
                            .tabs
                            .iter_mut()
                            .find(|tab| tab.id == destination && !tab.exited)
                        {
                            target
                                .terminal
                                .process_command(BackendCommand::Write(bytes.clone()));
                        }
                    }
                    self.paste_notice = "Paste sent after explicit confirmation.".into();
                    self.terminal_focus = Some(id);
                } else {
                    self.paste_notice =
                        "Paste cancelled because the terminal is no longer active.".into();
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
                        for pane in &mut self.workspace_panes {
                            if *pane == old_id {
                                *pane = new_id;
                            }
                        }
                        self.sync_input = SyncInputState::default();
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
