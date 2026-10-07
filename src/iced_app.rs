//! Production Iced application shell.
//!
//! This module intentionally reuses the existing validated Session/profile model.
//! Live PTY rendering is introduced separately so the GUI migration cannot bypass
//! host-key, argv, paste, process-lifecycle, or transfer safeguards.
use crate::{
    Session, load_sessions, save_session_edit, save_sessions, session_matches_query,
    session_profile_key, terminal,
};
use iced::widget::{
    button, center, column, container, mouse_area, opaque, operation, pane_grid, row, scrollable,
    space, stack, text, text_input,
};
use iced::futures::{SinkExt, Stream, StreamExt, channel::mpsc};
use iced::{Border, Color, Element, Fill, Font, Subscription, Task, Theme, event, keyboard};
use std::{path::PathBuf, sync::{Arc, Mutex}};

const BG: Color = Color::from_rgb8(14, 18, 25);
const PANEL: Color = Color::from_rgb8(22, 29, 40);
const ELEVATED: Color = Color::from_rgb8(30, 40, 55);
const LINE: Color = Color::from_rgb8(48, 62, 81);
const FG: Color = Color::from_rgb8(231, 237, 247);
const MUTED: Color = Color::from_rgb8(163, 177, 196);
const BLUE: Color = Color::from_rgb8(123, 176, 255);
const GREEN: Color = Color::from_rgb8(113, 217, 171);
const DANGER: Color = Color::from_rgb8(255, 138, 151);

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
    About,
}

#[derive(Clone, Debug)]
enum Message {
    Search(String),
    Open(usize),
    SelectTab(usize),
    New,
    Commands,
    About,
    CloseDialog,
    AskClose(usize),
    ConfirmClose(usize),
    ToggleFiles,
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
    Advanced,
    Submit,
    Scale(f64),
    PtyBridgeReady(PtyBridgeSender),
    PtyEvent(u64, egui_term::PtyEvent),
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
            advanced: false,
            error: None,
        }
    }
}

impl ConnectionForm {
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

        let mut session = Session {
            name: {
                let name = self.name.trim();
                if name.is_empty() {
                    host.to_owned()
                } else {
                    name.to_owned()
                }
            },
            host: host.to_owned(),
            user: self.user.trim().to_owned(),
            port,
            folder: self.folder.trim().to_owned(),
            ..Session::default()
        };

        // Keep the initial Iced form deliberately small. Advanced policy fields are
        // retained by the existing model and will be edited in the advanced panel.
        session.strict = false;
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
    form: ConnectionForm,
    dialog: Option<Dialog>,
    scale: f64,
    status: String,
    load_error: Option<String>,
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
            form: ConnectionForm::default(),
            dialog: None,
            scale: 1.0,
            status: "Ready".into(),
            load_error,
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
            pane.error = Some("Terminal event bridge is still initializing. Close and reopen this session.".into());
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
            Message::SelectTab(index) => {
                if index < self.tabs.len() {
                    self.active = index;
                }
            }
            Message::New => {
                self.form = ConnectionForm::default();
                self.dialog = Some(Dialog::Connection);
                return operation::focus("connection-host");
            }
            Message::Commands => self.dialog = Some(Dialog::Commands),
            Message::About => self.dialog = Some(Dialog::About),
            Message::CloseDialog => {
                self.dialog = None;
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
            Message::Advanced => self.form.advanced = !self.form.advanced,
            Message::Submit => {
                match self.form.session().and_then(|session| {
                    let profiles = save_session_edit(&self.profiles, None, session.clone())?;
                    save_sessions(&self.profiles_path, &profiles)?;
                    Ok((profiles, session))
                }) {
                    Ok((profiles, session)) => {
                        self.profiles = profiles;
                        let key = session_profile_key(&session);
                        if let Some(index) = self
                            .profiles
                            .iter()
                            .position(|profile| session_profile_key(profile) == key)
                        {
                            self.open(index);
                        }
                        self.form = ConnectionForm::default();
                        self.status =
                            "Profile saved using the existing validated profile store.".into();
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
            Message::Event(iced::Event::Keyboard(keyboard::Event::KeyPressed {
                key,
                modifiers,
                text,
                ..
            })) => {
                use keyboard::key::Named;

                if self.dialog.is_some() {
                    return match key.as_ref() {
                        keyboard::Key::Named(Named::Escape) => {
                            self.update(Message::CloseDialog)
                        }
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
                    keyboard::Key::Character("p")
                        if modifiers.command() && modifiers.shift() =>
                    {
                        return self.update(Message::Commands);
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
        let mut list = column![
            row![
                text("SESSIONS").size(12).color(MUTED),
                space::horizontal(),
                text(self.profiles.len().to_string()).size(12).color(MUTED)
            ],
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
            list = list.push(item);
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
                    text("The shell is now backed by the real profile store. Live terminal migration follows.")
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
            text("PROFILE READY").size(12).color(GREEN),
            space::horizontal(),
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
        pane_grid(&tab.panes, |id, profile, _| {
            let focused = tab.focus == id;
            let title = pane_grid::TitleBar::new(
                row![
                    text(if focused { "FOCUSED" } else { "SSH" })
                        .size(11)
                        .color(if focused { BLUE } else { MUTED }),
                    text(profile.name.clone()).size(13),
                    space::horizontal(),
                    action("x", Message::ClosePane(id)),
                ]
                .spacing(10)
                .align_y(iced::Center),
            )
            .padding([4, 10])
            .style(surface);

            let destination = if profile.user.is_empty() {
                profile.host.clone()
            } else {
                format!("{}@{}", profile.user, profile.host)
            };
            let port = profile
                .port
                .map(|port| port.to_string())
                .unwrap_or_else(|| "SSH config/default".into());
            let body = column![
                text("SESSION PROFILE").size(11).color(BLUE),
                text(destination).size(22),
                text(format!("Port: {port}")).size(13).color(MUTED),
                space::vertical().height(8),
                text("The Iced production shell is active. This pane deliberately does not start an SSH process until the frontend-neutral PTY bridge is connected, so the migration cannot bypass the existing terminal safety path.")
                    .size(13)
                    .color(MUTED),
                space::vertical().height(8),
                text("Next: attach the existing OpenSSH launch policy to a reusable PTY/parser backend, then render and drive it here.")
                    .size(13)
                    .color(MUTED),
            ]
            .spacing(12)
            .width(Fill);

            pane_grid::Content::new(
                container(scrollable(body).width(Fill).height(Fill))
                    .padding(18)
                    .width(Fill)
                    .height(Fill),
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
                let mut form = column![
                    text("New SSH connection").size(24),
                    text("Saved through the existing validated, non-secret profile store.")
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
                            text("Advanced policy is not duplicated into a second model. Identity, authentication policy, ProxyJump/proxy transport, ControlMaster, forwarding, keepalive and remote-command fields will edit the existing Session::ssh structure directly in the next UI pass.")
                                .size(13)
                                .color(MUTED),
                        )
                        .padding(12)
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
                        action("Save and open", Message::Submit).style(primary)
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
                action("Toggle files", Message::ToggleFiles).width(Fill),
                action("Close", Message::CloseDialog),
            ]
            .spacing(14)
            .into(),
            Dialog::Close(index) => column![
                text("Close this session?").size(24),
                text("This closes the Iced workspace tab. No SSH process is attached to this shell yet.")
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
            Dialog::About => column![
                text("Production Iced migration").size(24),
                text("This is no longer the fixture-only design preview. The shell reads and writes the real validated Inspirum profile store and uses the accepted WindTerm-style workspace structure.\n\nThe live SSH/PTY renderer is intentionally not attached in this commit because the current TerminalBackend is coupled to egui. The next migration step extracts that backend boundary while retaining OpenSSH host-key policy, paste safeguards and process lifecycle behaviour.")
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
    fn command_like_host_is_rejected_by_existing_policy() {
        let form = ConnectionForm {
            host: "-F".into(),
            ..Default::default()
        };
        assert!(form.session().is_err());
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
