//! Production Iced application shell.
//!
//! This module intentionally reuses the existing validated Session/profile model.
//! Live PTY rendering is introduced separately so the GUI migration cannot bypass
//! host-key, argv, paste, process-lifecycle, or transfer safeguards.
use crate::{
    Session, load_sessions, save_session_edit, save_sessions, session_matches_query,
    session_profile_key, terminal,
    terminal_ux::{self, PasteDecision, PastePolicy},
};
use iced::futures::{SinkExt, Stream, StreamExt, channel::mpsc};
use iced::widget::{
    button, center, column, container, mouse_area, opaque, operation, pane_grid, row, scrollable,
    sensor, space, stack, text, text_input,
};
use iced::{Border, Color, Element, Fill, Font, Subscription, Task, Theme, event, keyboard};
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
};

const BG: Color = Color::from_rgb8(14, 18, 25);
const PANEL: Color = Color::from_rgb8(22, 29, 40);
const ELEVATED: Color = Color::from_rgb8(30, 40, 55);
const LINE: Color = Color::from_rgb8(48, 62, 81);
const FG: Color = Color::from_rgb8(231, 237, 247);
const MUTED: Color = Color::from_rgb8(163, 177, 196);
const BLUE: Color = Color::from_rgb8(123, 176, 255);
const GREEN: Color = Color::from_rgb8(113, 217, 171);
const DANGER: Color = Color::from_rgb8(255, 138, 151);
const TERMINAL_CELL_WIDTH: f32 = 8.4;
const TERMINAL_CELL_HEIGHT: f32 = 18.0;

pub fn run(profiles_path: PathBuf, ssh_config: Option<PathBuf>) -> iced::Result {
    iced::application(
        move || App::boot(profiles_path.clone(), ssh_config.clone()),
        App::update,
        App::view,
    )
    .title("Inspirum Terminal")
    .theme(|_: &App| {
        Theme::custom(
            "Inspirum",
            iced::theme::Palette {
                background: BG,
                text: FG,
                primary: BLUE,
                success: GREEN,
                warning: Color::from_rgb8(240, 190, 100),
                danger: DANGER,
            },
        )
    })
    .settings(iced::Settings {
        default_text_size: iced::Pixels(14.0),
        ..Default::default()
    })
    .window(iced::window::Settings {
        size: iced::Size::new(1280.0, 800.0),
        min_size: Some(iced::Size::new(960.0, 640.0)),
        ..Default::default()
    })
    .scale_factor(|app: &App| app.scale as f32)
    .subscription(|_: &App| {
        Subscription::batch([
            event::listen_with(|event, _, _| match event {
                iced::Event::Keyboard(_) => Some(Message::Event(event)),
                _ => None,
            }),
            Subscription::run(pty_bridge),
        ])
    })
    .run()
}

type PtyBridgeSender = mpsc::UnboundedSender<(u64, egui_term::PtyEvent)>;

fn pty_bridge() -> impl Stream<Item = Message> {
    iced::stream::channel(100, async |mut output| {
        let (sender, mut receiver) = mpsc::unbounded();
        if output.send(Message::PtyBridgeReady(sender)).await.is_err() {
            return;
        }
        while let Some((id, event)) = receiver.next().await {
            if output.send(Message::PtyEvent(id, event)).await.is_err() {
                break;
            }
        }
    })
}

struct TerminalPane {
    id: u64,
    profile: Session,
    terminal: Option<egui_term::TerminalBackend>,
    transcript: String,
    error: Option<String>,
    exited: bool,
}

struct Workspace {
    profile: Session,
    panes: pane_grid::State<TerminalPane>,
    focus: pane_grid::Pane,
}

impl Workspace {
    fn new(profile: Session, terminal: TerminalPane) -> Self {
        let (panes, focus) = pane_grid::State::new(terminal);
        Self {
            profile,
            panes,
            focus,
        }
    }

    fn split(&mut self, axis: pane_grid::Axis, terminal: TerminalPane) {
        if self.panes.len() < 4 {
            if let Some((pane, _)) = self.panes.split(axis, self.focus, terminal) {
                self.focus = pane;
            }
        }
    }
}

#[derive(Clone, Copy)]
enum Dock {
    Terminal,
    Files,
}

#[derive(Clone)]
enum Dialog {
    Connection,
    Commands,
    Close(usize),
    PasteConfirm { id: u64, text: String },
    About,
}

#[derive(Clone, Debug)]
enum Message {
    Search(String),
    Open(usize),
    EditProfile(usize),
    SelectTab(usize),
    New,
    Commands,
    About,
    CloseDialog,
    AskClose(usize),
    ConfirmClose(usize),
    ToggleFiles,
    ToggleSidebar,
    RequestPaste,
    ClipboardRead(u64, Option<String>),
    ConfirmPaste,
    Reconnect(pane_grid::Pane),
    Split(pane_grid::Axis),
    Focus(pane_grid::Pane),
    Resize(pane_grid::ResizeEvent),
    Drag(pane_grid::DragEvent),
    ClosePane(pane_grid::Pane),
    DockResize(pane_grid::ResizeEvent),
    Name(String),
    Host(String),
    User(String),
    Port(String),
    Folder(String),
    IdentityFile(String),
    ProxyJump(String),
    ConnectTimeout(String),
    Keepalive(String),
    RemoteCommand(String),
    Advanced,
    Submit,
    Scale(f64),
    PtyBridgeReady(PtyBridgeSender),
    PtyEvent(u64, egui_term::PtyEvent),
    TerminalResized(u64, iced::Size),
    Event(iced::Event),
}

fn terminal_screen_text(terminal: &mut egui_term::TerminalBackend) -> String {
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

fn terminal_key_bytes(
    key: &keyboard::Key,
    modifiers: keyboard::Modifiers,
    committed_text: Option<&str>,
) -> Option<Vec<u8>> {
    use keyboard::key::Named;

    if modifiers.control() {
        if let keyboard::Key::Character(value) = key.as_ref() {
            let mut chars = value.chars();
            if let (Some(ch), None) = (chars.next(), chars.next()) {
                let ch = ch.to_ascii_lowercase();
                if ch.is_ascii_lowercase() {
                    return Some(vec![(ch as u8 - b'a') + 1]);
                }
                if ch == ' ' {
                    return Some(vec![0]);
                }
            }
        }
    }

    match key.as_ref() {
        keyboard::Key::Named(Named::Enter) => Some(vec![b'\r']),
        keyboard::Key::Named(Named::Tab) => Some(vec![b'\t']),
        keyboard::Key::Named(Named::Escape) => Some(vec![0x1b]),
        keyboard::Key::Named(Named::Backspace) => Some(vec![0x7f]),
        keyboard::Key::Named(Named::Delete) => Some(b"\x1b[3~".to_vec()),
        keyboard::Key::Named(Named::ArrowUp) => Some(b"\x1b[A".to_vec()),
        keyboard::Key::Named(Named::ArrowDown) => Some(b"\x1b[B".to_vec()),
        keyboard::Key::Named(Named::ArrowRight) => Some(b"\x1b[C".to_vec()),
        keyboard::Key::Named(Named::ArrowLeft) => Some(b"\x1b[D".to_vec()),
        keyboard::Key::Named(Named::Home) => Some(b"\x1b[H".to_vec()),
        keyboard::Key::Named(Named::End) => Some(b"\x1b[F".to_vec()),
        keyboard::Key::Named(Named::PageUp) => Some(b"\x1b[5~".to_vec()),
        keyboard::Key::Named(Named::PageDown) => Some(b"\x1b[6~".to_vec()),
        _ if !modifiers.control() && !modifiers.command() && !modifiers.alt() => committed_text
            .filter(|text| !text.is_empty())
            .map(|text| text.as_bytes().to_vec()),
        _ => None,
    }
}

struct ConnectionForm {
    name: String,
    host: String,
    user: String,
    port: String,
    folder: String,
    identity_file: String,
    proxy_jump: String,
    connect_timeout: String,
    keepalive: String,
    remote_command: String,
    base: Option<Session>,
    advanced: bool,
    error: Option<String>,
}

impl Default for ConnectionForm {
    fn default() -> Self {
        Self {
            name: String::new(),
            host: String::new(),
            user: String::new(),
            port: String::new(),
            folder: String::new(),
            identity_file: String::new(),
            proxy_jump: String::new(),
            connect_timeout: String::new(),
            keepalive: String::new(),
            remote_command: String::new(),
            base: None,
            advanced: false,
            error: None,
        }
    }
}

impl ConnectionForm {
    fn from_session(session: &Session) -> Self {
        Self {
            name: session.name.clone(),
            host: session.host.clone(),
            user: session.user.clone(),
            port: session
                .port
                .map(|value| value.to_string())
                .unwrap_or_default(),
            folder: session.folder.clone(),
            identity_file: session.ssh.identity_file.clone(),
            proxy_jump: session.ssh.proxy_jump.clone(),
            connect_timeout: session
                .ssh
                .connect_timeout_seconds
                .map(|value| value.to_string())
                .unwrap_or_default(),
            keepalive: session
                .ssh
                .server_alive_interval_seconds
                .map(|value| value.to_string())
                .unwrap_or_default(),
            remote_command: session.ssh.remote_command.clone(),
            base: Some(session.clone()),
            advanced: !session.ssh.identity_file.is_empty()
                || !session.ssh.proxy_jump.is_empty()
                || session.ssh.connect_timeout_seconds.is_some()
                || session.ssh.server_alive_interval_seconds.is_some()
                || !session.ssh.remote_command.is_empty(),
            error: None,
        }
    }

    fn optional_positive_u16(label: &str, value: &str) -> anyhow::Result<Option<u16>> {
        let value = value.trim();
        if value.is_empty() {
            return Ok(None);
        }
        let parsed = value
            .parse::<u16>()
            .map_err(|_| anyhow::anyhow!("{label} must be a number from 1 to 65535."))?;
        anyhow::ensure!(parsed > 0, "{label} must be a number from 1 to 65535.");
        Ok(Some(parsed))
    }

    fn session(&self) -> anyhow::Result<Session> {
        let host = self.host.trim();
        anyhow::ensure!(
            !host.is_empty(),
            "Enter a host name, SSH config alias, or IP address."
        );

        let port = if self.port.trim().is_empty() {
            None
        } else {
            let port = self
                .port
                .trim()
                .parse::<u16>()
                .map_err(|_| anyhow::anyhow!("Port must be a number from 1 to 65535."))?;
            anyhow::ensure!(port > 0, "Port must be a number from 1 to 65535.");
            Some(port)
        };

        let mut session = self.base.clone().unwrap_or_default();
        session.name = {
            let name = self.name.trim();
            if name.is_empty() {
                host.to_owned()
            } else {
                name.to_owned()
            }
        };
        session.host = host.to_owned();
        session.user = self.user.trim().to_owned();
        session.port = port;
        session.folder = self.folder.trim().to_owned();
        if self.base.is_none() {
            session.strict = false;
        }

        session.ssh.identity_file = self.identity_file.trim().to_owned();
        session.ssh.proxy_jump = self.proxy_jump.trim().to_owned();
        session.ssh.connect_timeout_seconds =
            Self::optional_positive_u16("Connect timeout", &self.connect_timeout)?;
        session.ssh.server_alive_interval_seconds =
            Self::optional_positive_u16("Server alive interval", &self.keepalive)?;
        session.ssh.remote_command = self.remote_command.trim().to_owned();

        session.ssh_args()?;
        Ok(session)
    }
}

struct App {
    profiles_path: PathBuf,
    ssh_config: Option<PathBuf>,
    profiles: Vec<Session>,
    query: String,
    tabs: Vec<Workspace>,
    active: usize,
    dock: pane_grid::State<Dock>,
    terminal_dock: pane_grid::Pane,
    files_dock: Option<pane_grid::Pane>,
    sidebar_collapsed: bool,
    form: ConnectionForm,
    editing_profile: Option<String>,
    dialog: Option<Dialog>,
    scale: f64,
    status: String,
    load_error: Option<String>,
    paste_policy: PastePolicy,
    pty_bridge: Option<PtyBridgeSender>,
    next_terminal_id: u64,
}

impl App {
    fn boot(profiles_path: PathBuf, ssh_config: Option<PathBuf>) -> Self {
        let (dock, terminal_dock) = pane_grid::State::new(Dock::Terminal);
        let (profiles, load_error) = match load_sessions(&profiles_path) {
            Ok(profiles) => (profiles, None),
            Err(error) => (
                Vec::new(),
                Some(format!("Could not load profiles: {error:#}")),
            ),
        };
        Self {
            profiles_path,
            ssh_config,
            profiles,
            query: String::new(),
            tabs: Vec::new(),
            active: 0,
            dock,
            terminal_dock,
            files_dock: None,
            sidebar_collapsed: false,
            form: ConnectionForm::default(),
            editing_profile: None,
            dialog: None,
            scale: 1.0,
            status: "Ready".into(),
            load_error,
            paste_policy: PastePolicy::ConfirmMultiline,
            pty_bridge: None,
            next_terminal_id: 1,
        }
    }

    fn new_terminal_pane(&mut self, profile: Session) -> TerminalPane {
        let id = self.next_terminal_id;
        self.next_terminal_id = self.next_terminal_id.saturating_add(1);

        let mut pane = TerminalPane {
            id,
            profile: profile.clone(),
            terminal: None,
            transcript: String::new(),
            error: None,
            exited: false,
        };

        let Some(bridge) = self.pty_bridge.clone() else {
            pane.error = Some(
                "Terminal event bridge is still initializing. Close and reopen this session."
                    .into(),
            );
            return pane;
        };

        let bridge = Arc::new(Mutex::new(bridge));
        let event_sink: Arc<dyn Fn(u64, egui_term::PtyEvent) + Send + Sync> =
            Arc::new(move |id, event| {
                if let Ok(sender) = bridge.lock() {
                    let _ = sender.unbounded_send((id, event));
                }
            });

        match terminal::connect_with_event_sink(
            id,
            &profile,
            self.ssh_config.as_deref(),
            event_sink,
        ) {
            Ok(terminal) => {
                pane.terminal = Some(terminal);
            }
            Err(error) => {
                pane.error = Some(format!("{error:#}"));
            }
        }
        pane
    }

    fn send_to_focused_terminal(&mut self, bytes: Vec<u8>) {
        let Some(tab) = self.tabs.get_mut(self.active) else {
            return;
        };
        let Some(pane) = tab.panes.get_mut(tab.focus) else {
            return;
        };
        if pane.exited {
            return;
        }
        if let Some(terminal) = pane.terminal.as_mut() {
            terminal.process_command(egui_term::BackendCommand::Write(bytes));
        }
    }

    fn focused_terminal_id(&self) -> Option<u64> {
        let tab = self.tabs.get(self.active)?;
        let pane = tab.panes.get(tab.focus)?;
        (!pane.exited && pane.terminal.is_some()).then_some(pane.id)
    }

    fn send_to_terminal(&mut self, id: u64, bytes: Vec<u8>) -> bool {
        for tab in &mut self.tabs {
            for (_, pane) in tab.panes.iter_mut() {
                if pane.id != id || pane.exited {
                    continue;
                }
                if let Some(terminal) = pane.terminal.as_mut() {
                    terminal.process_command(egui_term::BackendCommand::Write(bytes));
                    return true;
                }
                return false;
            }
        }
        false
    }

    fn refresh_terminal(&mut self, id: u64, exited: bool) {
        for tab in &mut self.tabs {
            for (_, pane) in tab.panes.iter_mut() {
                if pane.id != id {
                    continue;
                }
                if let Some(terminal) = pane.terminal.as_mut() {
                    pane.transcript = terminal_screen_text(terminal);
                }
                if exited {
                    pane.exited = true;
                }
                return;
            }
        }
    }

    fn resize_terminal(&mut self, id: u64, size: iced::Size) {
        let width = (size.width - 28.0).max(1.0);
        let height = (size.height - 28.0).max(1.0);
        for tab in &mut self.tabs {
            for (_, pane) in tab.panes.iter_mut() {
                if pane.id != id {
                    continue;
                }
                if let Some(terminal) = pane.terminal.as_mut() {
                    terminal.process_command(egui_term::BackendCommand::Resize(
                        egui_term::Size::new(width, height),
                        egui_term::Size::new(TERMINAL_CELL_WIDTH, TERMINAL_CELL_HEIGHT),
                    ));
                }
                return;
            }
        }
    }

    fn open(&mut self, index: usize) {
        if let Some(profile) = self.profiles.get(index).cloned() {
            let key = session_profile_key(&profile);
            if let Some(existing) = self
                .tabs
                .iter()
                .position(|tab| session_profile_key(&tab.profile) == key)
            {
                self.active = existing;
            } else {
                let terminal = self.new_terminal_pane(profile.clone());
                self.tabs.push(Workspace::new(profile, terminal));
                self.active = self.tabs.len() - 1;
            }
            self.status = "SSH session opened through the production OpenSSH/PTY backend.".into();
        }
        self.dialog = None;
    }

    fn toggle_files(&mut self) {
        if let Some(pane) = self.files_dock.take() {
            self.dock.close(pane);
        } else if !self.tabs.is_empty() {
            if let Some((pane, split)) =
                self.dock
                    .split(pane_grid::Axis::Horizontal, self.terminal_dock, Dock::Files)
            {
                self.files_dock = Some(pane);
                self.dock.resize(split, 0.62);
            }
        }
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Search(value) => {
                if self.dialog.is_none() {
                    self.query = value;
                }
            }
            Message::Open(index) => self.open(index),
            Message::EditProfile(index) => {
                if let Some(profile) = self.profiles.get(index).cloned() {
                    self.editing_profile = Some(session_profile_key(&profile));
                    self.form = ConnectionForm::from_session(&profile);
                    self.dialog = Some(Dialog::Connection);
                    return operation::focus("connection-name");
                }
            }
            Message::SelectTab(index) => {
                if index < self.tabs.len() {
                    self.active = index;
                }
            }
            Message::New => {
                self.editing_profile = None;
                self.form = ConnectionForm::default();
                self.dialog = Some(Dialog::Connection);
                return operation::focus("connection-host");
            }
            Message::Commands => self.dialog = Some(Dialog::Commands),
            Message::About => self.dialog = Some(Dialog::About),
            Message::CloseDialog => {
                self.dialog = None;
                self.editing_profile = None;
                self.form = ConnectionForm::default();
            }
            Message::AskClose(index) => self.dialog = Some(Dialog::Close(index)),
            Message::ConfirmClose(index) => {
                if index < self.tabs.len() {
                    self.tabs.remove(index);
                    if index < self.active {
                        self.active -= 1;
                    }
                    self.active = self.active.min(self.tabs.len().saturating_sub(1));
                }
                if self.tabs.is_empty() && self.files_dock.is_some() {
                    self.toggle_files();
                }
                self.dialog = None;
            }
            Message::ToggleFiles => {
                self.dialog = None;
                self.toggle_files();
            }
            Message::ToggleSidebar => {
                self.dialog = None;
                self.sidebar_collapsed = !self.sidebar_collapsed;
            }
            Message::RequestPaste => {
                let Some(id) = self.focused_terminal_id() else {
                    self.status = "Focus a connected terminal before pasting.".into();
                    return Task::none();
                };
                return iced::clipboard::read()
                    .map(move |contents| Message::ClipboardRead(id, contents));
            }
            Message::ClipboardRead(id, contents) => {
                let Some(text) = contents else {
                    self.status = "Clipboard does not contain text.".into();
                    return Task::none();
                };
                if text.is_empty() {
                    self.status = "Clipboard text is empty.".into();
                    return Task::none();
                }
                match terminal_ux::classify_paste(self.paste_policy, &text) {
                    PasteDecision::Send => {
                        if self.send_to_terminal(id, text.into_bytes()) {
                            self.status = "Clipboard text pasted to the focused terminal.".into();
                        } else {
                            self.status =
                                "Paste cancelled because that terminal is no longer active.".into();
                        }
                    }
                    PasteDecision::Confirm => {
                        self.dialog = Some(Dialog::PasteConfirm { id, text });
                        self.status =
                            "Multiline paste is waiting for explicit confirmation.".into();
                    }
                    PasteDecision::Block => {
                        self.status =
                            "Paste blocked by policy (multiline or NUL-containing payload).".into();
                    }
                }
            }
            Message::ConfirmPaste => {
                if let Some(Dialog::PasteConfirm { id, text }) = self.dialog.take() {
                    if self.send_to_terminal(id, text.into_bytes()) {
                        self.status = "Paste sent after explicit confirmation.".into();
                    } else {
                        self.status =
                            "Paste cancelled because that terminal is no longer active.".into();
                    }
                }
            }
            Message::Reconnect(pane_id) => {
                let profile = self
                    .tabs
                    .get(self.active)
                    .and_then(|tab| tab.panes.get(pane_id))
                    .map(|pane| pane.profile.clone());

                if let Some(profile) = profile {
                    let replacement = self.new_terminal_pane(profile);
                    let connected = replacement.terminal.is_some() && replacement.error.is_none();
                    if let Some(tab) = self.tabs.get_mut(self.active)
                        && let Some(pane) = tab.panes.get_mut(pane_id)
                    {
                        *pane = replacement;
                        tab.focus = pane_id;
                        self.status = if connected {
                            "Reconnected through the production OpenSSH/PTY backend.".into()
                        } else {
                            "Reconnect attempt failed; review the pane error.".into()
                        };
                    }
                }
            }
            Message::Split(axis) => {
                self.dialog = None;
                if let Some(profile) = self.tabs.get(self.active).map(|tab| tab.profile.clone()) {
                    let terminal = self.new_terminal_pane(profile);
                    if let Some(tab) = self.tabs.get_mut(self.active) {
                        tab.split(axis, terminal);
                    }
                }
            }
            Message::Focus(pane) => {
                if let Some(tab) = self.tabs.get_mut(self.active) {
                    tab.focus = pane;
                }
            }
            Message::Resize(event) => {
                if let Some(tab) = self.tabs.get_mut(self.active) {
                    tab.panes.resize(event.split, event.ratio.clamp(0.18, 0.82));
                }
            }
            Message::Drag(pane_grid::DragEvent::Dropped { pane, target }) => {
                if let Some(tab) = self.tabs.get_mut(self.active) {
                    tab.panes.drop(pane, target);
                }
            }
            Message::Drag(_) => {}
            Message::ClosePane(pane) => {
                if let Some(tab) = self.tabs.get_mut(self.active) {
                    if tab.panes.len() == 1 {
                        self.dialog = Some(Dialog::Close(self.active));
                    } else if let Some((_, sibling)) = tab.panes.close(pane) {
                        tab.focus = sibling;
                    }
                }
            }
            Message::DockResize(event) => {
                self.dock.resize(event.split, event.ratio.clamp(0.30, 0.75));
            }
            Message::Name(value) => self.form.name = value,
            Message::Host(value) => self.form.host = value,
            Message::User(value) => self.form.user = value,
            Message::Port(value) => self.form.port = value,
            Message::Folder(value) => self.form.folder = value,
            Message::IdentityFile(value) => self.form.identity_file = value,
            Message::ProxyJump(value) => self.form.proxy_jump = value,
            Message::ConnectTimeout(value) => self.form.connect_timeout = value,
            Message::Keepalive(value) => self.form.keepalive = value,
            Message::RemoteCommand(value) => self.form.remote_command = value,
            Message::Advanced => self.form.advanced = !self.form.advanced,
            Message::Submit => {
                let selected_profile = self.editing_profile.clone();
                let was_editing = selected_profile.is_some();
                match self.form.session().and_then(|session| {
                    let profiles = save_session_edit(
                        &self.profiles,
                        selected_profile.as_deref(),
                        session.clone(),
                    )?;
                    save_sessions(&self.profiles_path, &profiles)?;
                    Ok((profiles, session))
                }) {
                    Ok((profiles, session)) => {
                        self.profiles = profiles;
                        self.dialog = None;
                        self.editing_profile = None;
                        self.form = ConnectionForm::default();
                        if was_editing {
                            self.status =
                                "Profile changes saved. Existing live SSH panes keep their current connection until reconnect.".into();
                        } else {
                            let key = session_profile_key(&session);
                            if let Some(index) = self
                                .profiles
                                .iter()
                                .position(|profile| session_profile_key(profile) == key)
                            {
                                self.open(index);
                            }
                            self.status =
                                "Profile saved and opened using the existing validated profile store.".into();
                        }
                    }
                    Err(error) => self.form.error = Some(format!("{error:#}")),
                }
            }
            Message::Scale(delta) => self.scale = (self.scale + delta).clamp(0.85, 1.50),
            Message::PtyBridgeReady(sender) => {
                self.pty_bridge = Some(sender);
                self.status = "Terminal event bridge ready.".into();
            }
            Message::PtyEvent(id, event) => {
                let exited = matches!(event, egui_term::PtyEvent::Exit);
                self.refresh_terminal(id, exited);
                if exited {
                    self.status = format!("Terminal {id} exited.");
                }
            }
            Message::TerminalResized(id, size) => self.resize_terminal(id, size),
            Message::Event(iced::Event::Keyboard(keyboard::Event::KeyPressed {
                key,
                modifiers,
                text,
                ..
            })) => {
                use keyboard::key::Named;

                if self.dialog.is_some() {
                    if matches!(self.dialog, Some(Dialog::PasteConfirm { .. }))
                        && matches!(key.as_ref(), keyboard::Key::Named(Named::Enter))
                    {
                        return self.update(Message::ConfirmPaste);
                    }
                    return match key.as_ref() {
                        keyboard::Key::Named(Named::Escape) => self.update(Message::CloseDialog),
                        keyboard::Key::Named(Named::Tab) => {
                            if modifiers.shift() {
                                operation::focus_previous()
                            } else {
                                operation::focus_next()
                            }
                        }
                        _ => Task::none(),
                    };
                }

                match key.as_ref() {
                    keyboard::Key::Character("n") if modifiers.command() => {
                        return self.update(Message::New);
                    }
                    keyboard::Key::Character("p") if modifiers.command() && modifiers.shift() => {
                        return self.update(Message::Commands);
                    }
                    keyboard::Key::Character("v") if modifiers.command() => {
                        return self.update(Message::RequestPaste);
                    }
                    _ => {}
                }

                if let Some(bytes) =
                    terminal_key_bytes(&key, modifiers, text.as_ref().map(|value| value.as_str()))
                {
                    self.send_to_focused_terminal(bytes);
                }
            }
            Message::Event(_) => {}
        }
        Task::none()
    }

    fn view(&self) -> Element<'_, Message> {
        let header = container(
            row![
                container(text(">_").size(21).color(BLUE))
                    .padding([3, 8])
                    .style(card),
                text("Inspirum").size(19),
                text("Production Iced shell").size(12).color(MUTED),
                space::horizontal(),
                action("Commands", Message::Commands),
                action("A-", Message::Scale(-0.1)),
                action("A+", Message::Scale(0.1)),
                action("About", Message::About),
            ]
            .spacing(12)
            .align_y(iced::Center),
        )
        .padding([10, 16])
        .style(surface);

        let footer = container(
            row![
                text("ICED").size(12).color(BLUE),
                text(&self.status).size(12).color(MUTED),
                space::horizontal(),
                text(format!(
                    "{} saved profiles{}",
                    self.profiles.len(),
                    if self.ssh_config.is_some() {
                        "  /  custom SSH config"
                    } else {
                        ""
                    }
                ))
                .size(12)
                .color(MUTED),
            ]
            .spacing(12)
            .align_y(iced::Center),
        )
        .padding([9, 16])
        .style(surface);

        let mut body = column![
            header,
            row![self.sidebar(), self.workspace()].height(Fill),
            footer
        ]
        .height(Fill);

        if let Some(error) = &self.load_error {
            body = body.push(
                container(text(error).size(12).color(DANGER))
                    .padding([8, 16])
                    .width(Fill)
                    .style(surface),
            );
        }

        if let Some(dialog) = &self.dialog {
            let content = self.dialog_view(dialog);
            stack![
                body,
                opaque(
                    mouse_area(center(opaque(content)).style(|_| container::Style {
                        background: Some(Color::from_rgba(0.02, 0.03, 0.05, 0.82).into()),
                        ..Default::default()
                    }))
                    .on_press(Message::CloseDialog)
                )
            ]
            .into()
        } else {
            body.into()
        }
    }

    fn sidebar(&self) -> Element<'_, Message> {
        if self.sidebar_collapsed {
            return container(
                column![
                    action(">>", Message::ToggleSidebar).width(Fill),
                    action("+", Message::New).style(primary).width(Fill),
                    space::vertical().height(6),
                    text("SSH").size(10).color(BLUE),
                    space::vertical(),
                    text(self.profiles.len().to_string()).size(11).color(MUTED),
                ]
                .spacing(10)
                .align_x(iced::Center),
            )
            .width(58)
            .height(Fill)
            .padding([14, 8])
            .style(surface)
            .into();
        }

        let mut list = column![
            row![
                text("SESSIONS").size(12).color(MUTED),
                space::horizontal(),
                text(self.profiles.len().to_string()).size(12).color(MUTED),
                action("<<", Message::ToggleSidebar),
            ]
            .spacing(6)
            .align_y(iced::Center),
            action("+  New connection", Message::New)
                .style(primary)
                .width(Fill),
            text_input("Search sessions...", &self.query)
                .id("session-search")
                .on_input_maybe(self.dialog.is_none().then_some(Message::Search))
                .padding(10),
            space::vertical().height(6),
        ]
        .spacing(12);

        let mut current_group = String::new();
        let mut count = 0;
        for (index, profile) in self
            .profiles
            .iter()
            .enumerate()
            .filter(|(_, profile)| session_matches_query(profile, &self.query))
        {
            let group = if profile.folder.trim().is_empty() {
                "Ungrouped"
            } else {
                profile.folder.as_str()
            };
            if current_group != group {
                list = list
                    .push(container(text(group.to_owned()).size(12).color(MUTED)).padding([8, 2]));
                current_group = group.to_owned();
            }

            let selected = self.tabs.get(self.active).is_some_and(|tab| {
                session_profile_key(&tab.profile) == session_profile_key(profile)
            });
            let destination = if profile.user.is_empty() {
                profile.host.clone()
            } else {
                format!("{}@{}", profile.user, profile.host)
            };
            let item = button(
                column![
                    row![
                        text(profile.name.clone()).size(14),
                        space::horizontal(),
                        if profile.favorite {
                            text("PIN").size(10).color(GREEN)
                        } else {
                            text("SSH").size(10).color(BLUE)
                        }
                    ]
                    .spacing(4),
                    text(destination).size(12).color(MUTED),
                ]
                .spacing(5),
            )
            .padding(10)
            .width(Fill)
            .on_press(Message::Open(index))
            .style(if selected { selected_button } else { quiet });
            list = list.push(
                row![item, action("Edit", Message::EditProfile(index))]
                    .spacing(4)
                    .align_y(iced::Center),
            );
            count += 1;
        }

        if count == 0 {
            list = list.push(text("No matching saved sessions").color(MUTED));
        }

        container(
            column![
                scrollable(list).height(Fill),
                text("Saved profiles are read from the existing Inspirum profile store.")
                    .size(12)
                    .color(MUTED)
            ]
            .spacing(16),
        )
        .width(270)
        .height(Fill)
        .padding(16)
        .style(surface)
        .into()
    }

    fn workspace(&self) -> Element<'_, Message> {
        if self.tabs.is_empty() {
            return center(
                column![
                    text(">_").size(56).color(BLUE),
                    text("Your workspace, ready.").size(30),
                    text("Open a saved session or create a new connection.")
                        .size(16)
                        .color(MUTED),
                    space::vertical().height(12),
                    action("+  New connection", Message::New).style(primary),
                    space::vertical().height(12),
                    text("The Iced shell now uses the production OpenSSH PTY backend. Open a profile to connect.")
                        .size(13)
                        .color(MUTED),
                ]
                .spacing(12)
                .align_x(iced::Center),
            )
            .into();
        }

        let mut tabs = row![].spacing(4);
        for (index, tab) in self.tabs.iter().enumerate() {
            tabs = tabs.push(
                container(
                    row![
                        action(tab.profile.name.clone(), Message::SelectTab(index)).style(
                            if index == self.active {
                                selected_button
                            } else {
                                quiet
                            }
                        ),
                        action("x", Message::AskClose(index)),
                    ]
                    .spacing(0),
                )
                .style(if index == self.active {
                    active_card
                } else {
                    surface
                }),
            );
        }
        tabs = tabs.push(action("+", Message::New));

        let toolbar = row![
            text("LIVE SSH").size(12).color(GREEN),
            space::horizontal(),
            action("Paste", Message::RequestPaste),
            action("Split right", Message::Split(pane_grid::Axis::Vertical)),
            action("Split down", Message::Split(pane_grid::Axis::Horizontal)),
            action(
                if self.files_dock.is_some() {
                    "Hide files"
                } else {
                    "Files"
                },
                Message::ToggleFiles
            ),
        ]
        .spacing(8)
        .align_y(iced::Center);

        let dock = pane_grid(&self.dock, |_, panel, _| {
            pane_grid::Content::new(match panel {
                Dock::Terminal => self.terminals(),
                Dock::Files => self.files(),
            })
        })
        .spacing(6)
        .on_resize(8, Message::DockResize)
        .height(Fill);

        container(
            column![
                scrollable(tabs).direction(scrollable::Direction::Horizontal(
                    scrollable::Scrollbar::default()
                )),
                toolbar,
                dock
            ]
            .spacing(10),
        )
        .padding(14)
        .height(Fill)
        .width(Fill)
        .into()
    }

    fn terminals(&self) -> Element<'_, Message> {
        let tab = &self.tabs[self.active];
        pane_grid(&tab.panes, |id, pane, _| {
            let focused = tab.focus == id;
            let state = if pane.exited {
                ("EXITED", MUTED)
            } else if pane.error.is_some() {
                ("ERROR", DANGER)
            } else if pane.terminal.is_some() {
                ("CONNECTED", GREEN)
            } else {
                ("CONNECTING", BLUE)
            };
            let title = pane_grid::TitleBar::new(
                row![
                    text(if focused { "FOCUSED" } else { "SSH" })
                        .size(11)
                        .color(if focused { BLUE } else { MUTED }),
                    text(pane.profile.name.clone()).size(13),
                    text(state.0).size(10).color(state.1),
                    space::horizontal(),
                    if pane.exited || pane.error.is_some() {
                        action("Reconnect", Message::Reconnect(id))
                    } else {
                        action("Paste", Message::RequestPaste)
                    },
                    action("x", Message::ClosePane(id)),
                ]
                .spacing(10)
                .align_y(iced::Center),
            )
            .padding([4, 10])
            .style(surface);

            let terminal_body: Element<'_, Message> = if let Some(error) = &pane.error {
                column![
                    text("Unable to start SSH session").size(16).color(DANGER),
                    text(error).size(13).color(MUTED),
                    space::vertical().height(8),
                    text("The saved profile was not altered.")
                        .size(12)
                        .color(MUTED),
                ]
                .spacing(10)
                .width(Fill)
                .into()
            } else {
                let transcript = if pane.transcript.is_empty() {
                    if pane.exited {
                        "Session exited without terminal output."
                    } else {
                        "Connecting with system OpenSSH..."
                    }
                } else {
                    pane.transcript.as_str()
                };
                scrollable(text(transcript).font(Font::MONOSPACE).size(15).color(FG))
                    .width(Fill)
                    .height(Fill)
                    .into()
            };

            pane_grid::Content::new(
                sensor(
                    container(terminal_body)
                        .padding(14)
                        .width(Fill)
                        .height(Fill),
                )
                .key(pane.id)
                .on_resize(move |size| Message::TerminalResized(pane.id, size)),
            )
            .title_bar(title)
            .style(if focused { active_card } else { card })
        })
        .spacing(8)
        .on_click(Message::Focus)
        .on_drag(Message::Drag)
        .on_resize(8, Message::Resize)
        .height(Fill)
        .into()
    }

    fn files(&self) -> Element<'_, Message> {
        let profile = &self.tabs[self.active].profile;
        let target = if profile.user.is_empty() {
            profile.host.clone()
        } else {
            format!("{}@{}", profile.user, profile.host)
        };
        container(
            column![
                row![
                    text("Files").size(16),
                    text(format!("Target: {target}")).size(12).color(BLUE),
                    space::horizontal(),
                    text("SFTP bridge pending").size(12).color(MUTED)
                ]
                .spacing(12)
                .align_y(iced::Center),
                container(
                    column![
                        text("No fixture files are shown in the production shell.")
                            .size(14),
                        text("The existing SFTP/transfer implementation will be attached here after the live terminal bridge, preserving per-session ownership and transfer safeguards.")
                            .size(13)
                            .color(MUTED),
                    ]
                    .spacing(10)
                )
                .padding(16)
                .width(Fill)
                .height(Fill)
                .style(card),
            ]
            .spacing(10)
            .height(Fill),
        )
        .padding(12)
        .height(Fill)
        .width(Fill)
        .style(card)
        .into()
    }

    fn dialog_view(&self, dialog: &Dialog) -> Element<'_, Message> {
        let body: Element<'_, Message> = match dialog {
            Dialog::Connection => {
                let editing = self.editing_profile.is_some();
                let mut form = column![
                    text(if editing {
                        "Edit SSH connection"
                    } else {
                        "New SSH connection"
                    })
                    .size(24),
                    text(if editing {
                        "Changes are saved to the existing profile. Live panes keep their current connection until reconnect."
                    } else {
                        "Saved through the existing validated, non-secret profile store."
                    })
                    .color(MUTED),
                    field(
                        "Session name",
                        "Optional; defaults to host",
                        &self.form.name,
                        Message::Name
                    ),
                    field(
                        "Host",
                        "server.example, SSH alias, or IP",
                        &self.form.host,
                        Message::Host
                    ),
                    row![
                        field(
                            "Username",
                            "Inherit if empty",
                            &self.form.user,
                            Message::User
                        ),
                        container(field(
                            "Port",
                            "inherit / 22",
                            &self.form.port,
                            Message::Port
                        ))
                        .width(140)
                    ]
                    .spacing(14),
                    field(
                        "Folder",
                        "Optional group",
                        &self.form.folder,
                        Message::Folder
                    ),
                    action(
                        if self.form.advanced {
                            "-  Advanced SSH options"
                        } else {
                            "+  Advanced SSH options"
                        },
                        Message::Advanced
                    ),
                ]
                .spacing(16);

                if self.form.advanced {
                    form = form.push(
                        container(
                            column![
                                field(
                                    "Identity file",
                                    "~/.ssh/id_ed25519 or other key path",
                                    &self.form.identity_file,
                                    Message::IdentityFile
                                ),
                                field(
                                    "ProxyJump",
                                    "bastion or user@bastion:2222",
                                    &self.form.proxy_jump,
                                    Message::ProxyJump
                                ),
                                row![
                                    field(
                                        "Connect timeout",
                                        "seconds / inherit",
                                        &self.form.connect_timeout,
                                        Message::ConnectTimeout
                                    ),
                                    field(
                                        "Server alive interval",
                                        "seconds / inherit",
                                        &self.form.keepalive,
                                        Message::Keepalive
                                    )
                                ]
                                .spacing(14),
                                field(
                                    "Remote command",
                                    "Optional command after authentication",
                                    &self.form.remote_command,
                                    Message::RemoteCommand
                                ),
                                text("These values write directly into the existing Session::ssh model and use its current validation. Advanced settings not exposed here yet are preserved when editing an existing profile.")
                                    .size(12)
                                    .color(MUTED),
                            ]
                            .spacing(14)
                        )
                        .padding(14)
                        .style(card),
                    );
                }
                if let Some(error) = &self.form.error {
                    form = form.push(text(error).color(DANGER));
                }
                form.push(
                    row![
                        space::horizontal(),
                        action("Cancel", Message::CloseDialog),
                        action(
                            if editing { "Save changes" } else { "Save and open" },
                            Message::Submit
                        )
                        .style(primary)
                    ]
                    .spacing(10),
                )
                .into()
            }
            Dialog::Commands => column![
                text("Commands").size(24),
                text("Workspace actions. Nothing is sent to a remote host yet.").color(MUTED),
                action(
                    "New connection                      Ctrl / Cmd + N",
                    Message::New
                )
                .width(Fill),
                action(
                    "Split active pane right",
                    Message::Split(pane_grid::Axis::Vertical)
                )
                .width(Fill),
                action(
                    if self.sidebar_collapsed {
                        "Show session library"
                    } else {
                        "Hide session library"
                    },
                    Message::ToggleSidebar
                )
                .width(Fill),
                action("Toggle files", Message::ToggleFiles).width(Fill),
                action("Close", Message::CloseDialog),
            ]
            .spacing(14)
            .into(),
            Dialog::Close(index) => column![
                text("Close this session?").size(24),
                text("This disconnects the live SSH/PTY session and closes its Iced workspace tab.")
                    .color(MUTED),
                row![
                    space::horizontal(),
                    action("Cancel", Message::CloseDialog),
                    action("Close session", Message::ConfirmClose(*index)).style(button::danger)
                ]
                .spacing(10),
            ]
            .spacing(20)
            .into(),
            Dialog::PasteConfirm { id: _, text: paste } => {
                let line_count = paste
                    .as_bytes()
                    .iter()
                    .filter(|&&byte| matches!(byte, b'\r' | b'\n'))
                    .count()
                    + 1;
                let truncated = paste.chars().count() > 4000;
                let mut preview = paste.chars().take(4000).collect::<String>();
                if truncated {
                    preview.push_str("\n… preview truncated …");
                }
                column![
                    text("Confirm terminal paste").size(24),
                    text(format!(
                        "Paste {} bytes across approximately {} line(s)?",
                        paste.len(),
                        line_count
                    )),
                    text("Review carefully. Multiline terminal pastes can execute several commands immediately.")
                        .size(13)
                        .color(MUTED),
                    container(
                        scrollable(text(preview).font(Font::MONOSPACE).size(13))
                            .height(220)
                    )
                    .padding(12)
                    .width(Fill)
                    .style(card),
                    row![
                        space::horizontal(),
                        action("Cancel", Message::CloseDialog),
                        action("Paste now", Message::ConfirmPaste).style(primary)
                    ]
                    .spacing(10),
                    text("Keyboard: Enter confirms; Escape cancels.")
                        .size(12)
                        .color(MUTED),
                ]
                .spacing(16)
                .into()
            }
            Dialog::About => column![
                text("Production Iced migration").size(24),
                text("This is no longer the fixture-only design preview. The shell reads and writes the real validated Inspirum profile store, opens live SSH sessions through the existing OpenSSH/PTY backend, and uses the accepted WindTerm-style workspace structure.\n\nClipboard paste now reuses the existing terminal safety policy: NUL-containing payloads are blocked and multiline paste requires explicit review and confirmation. Exited or failed panes can reconnect in place from their pane header. Terminal rendering fidelity and real SFTP integration remain migration work.")
                    .color(MUTED),
                action("Back to workspace", Message::CloseDialog).style(primary),
            ]
            .spacing(20)
            .into(),
        };

        container(scrollable(body))
            .width(600)
            .max_height(720)
            .padding(28)
            .style(active_card)
            .into()
    }
}

fn field<'a>(
    label: &'a str,
    hint: &'a str,
    value: &'a str,
    message: fn(String) -> Message,
) -> Element<'a, Message> {
    let id = match label {
        "Host" => "connection-host",
        "Username" => "connection-user",
        "Port" => "connection-port",
        "Folder" => "connection-folder",
        "Identity file" => "connection-identity-file",
        "ProxyJump" => "connection-proxy-jump",
        "Connect timeout" => "connection-timeout",
        "Server alive interval" => "connection-keepalive",
        "Remote command" => "connection-remote-command",
        _ => "connection-name",
    };
    column![
        text(label).size(13).color(MUTED),
        text_input(hint, value)
            .id(id)
            .on_input(message)
            .on_submit(Message::Submit)
            .padding(11)
    ]
    .spacing(7)
    .into()
}

fn action<'a>(label: impl Into<String>, message: Message) -> button::Button<'a, Message> {
    button(text(label.into()).size(14))
        .padding([8, 12])
        .style(quiet)
        .on_press(message)
}

fn surface(_: &Theme) -> container::Style {
    container::Style {
        background: Some(PANEL.into()),
        text_color: Some(FG),
        ..Default::default()
    }
}

fn card(_: &Theme) -> container::Style {
    container::Style {
        background: Some(BG.into()),
        text_color: Some(FG),
        border: Border {
            color: LINE,
            width: 1.0,
            radius: 6.0.into(),
        },
        ..Default::default()
    }
}

fn active_card(theme: &Theme) -> container::Style {
    container::Style {
        border: Border {
            color: BLUE,
            width: 1.0,
            radius: 6.0.into(),
        },
        ..card(theme)
    }
}

fn quiet(_: &Theme, status: button::Status) -> button::Style {
    button::Style {
        background: if matches!(status, button::Status::Hovered | button::Status::Pressed) {
            Some(ELEVATED.into())
        } else {
            None
        },
        text_color: if status == button::Status::Disabled {
            MUTED
        } else {
            FG
        },
        border: Border {
            radius: 5.0.into(),
            ..Default::default()
        },
        ..Default::default()
    }
}

fn selected_button(theme: &Theme, status: button::Status) -> button::Style {
    button::Style {
        background: Some(ELEVATED.into()),
        text_color: BLUE,
        ..quiet(theme, status)
    }
}

fn primary(theme: &Theme, status: button::Status) -> button::Style {
    button::Style {
        background: Some(BLUE.into()),
        text_color: BG,
        ..quiet(theme, status)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connection_form_uses_production_session_validation() {
        let form = ConnectionForm {
            host: "server.example".into(),
            user: "ops".into(),
            ..Default::default()
        };
        let session = form.session().expect("valid session");
        assert_eq!(session.name, "server.example");
        assert_eq!(session.host, "server.example");
        assert_eq!(session.user, "ops");
        assert_eq!(session.port, None);
        assert!(!session.strict);
    }

    #[test]
    fn editing_form_preserves_unexposed_ssh_options() {
        let mut session = Session {
            name: "server".into(),
            host: "server.example".into(),
            ..Session::default()
        };
        session.ssh.password_auth = Some(false);
        session.ssh.agent_forwarding = Some(true);
        session.ssh.identity_file = "/tmp/key".into();

        let mut form = ConnectionForm::from_session(&session);
        form.identity_file = "/tmp/new-key".into();
        let updated = form.session().expect("edited session");

        assert_eq!(updated.ssh.identity_file, "/tmp/new-key");
        assert_eq!(updated.ssh.password_auth, Some(false));
        assert_eq!(updated.ssh.agent_forwarding, Some(true));
    }

    #[test]
    fn command_like_host_is_rejected_by_existing_policy() {
        let form = ConnectionForm {
            host: "-F".into(),
            ..Default::default()
        };
        assert!(form.session().is_err());
    }

    #[test]
    fn iced_paste_keeps_existing_multiline_and_nul_policy() {
        assert_eq!(
            terminal_ux::classify_paste(PastePolicy::ConfirmMultiline, "echo safe"),
            PasteDecision::Send
        );
        assert_eq!(
            terminal_ux::classify_paste(PastePolicy::ConfirmMultiline, "echo first\necho second"),
            PasteDecision::Confirm
        );
        assert_eq!(
            terminal_ux::classify_paste(PastePolicy::ConfirmMultiline, "bad\0payload"),
            PasteDecision::Block
        );
    }

    #[test]
    fn port_is_optional_but_zero_is_not() {
        let inherited = ConnectionForm {
            host: "server.example".into(),
            ..Default::default()
        };
        assert_eq!(inherited.session().unwrap().port, None);

        let zero = ConnectionForm {
            host: "server.example".into(),
            port: "0".into(),
            ..Default::default()
        };
        assert!(zero.session().is_err());
    }
}
