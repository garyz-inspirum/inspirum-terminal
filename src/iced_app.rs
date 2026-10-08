//! Production Iced application shell.
//!
//! This module intentionally reuses the existing validated Session/profile model.
//! Live PTY rendering is introduced separately so the GUI migration cannot bypass
//! host-key, argv, paste, process-lifecycle, or transfer safeguards.
use crate::{
    ProxyAuth, ProxyKind, Session, SessionImportMode,
    command_palette::{self, PaletteItem, Snippet, SnippetLibrary},
    import_sessions, load_sessions,
    remote_edit::{self, SaveOutcome},
    save_session_edit, save_sessions, session_matches_query, session_profile_key, sftp, terminal,
    terminal_ux::{self, PasteDecision, PastePolicy},
};
use iced::futures::{SinkExt, Stream, StreamExt, channel::mpsc};
use iced::widget::{
    button, canvas, center, column, container, mouse_area, opaque, operation, pane_grid, row,
    scrollable, sensor, space, stack, text, text_editor, text_input,
};
use iced::{
    Border, Color, Element, Fill, Font, Subscription, Task, Theme, event, font, keyboard, mouse,
};
use std::{
    cell::{Cell, RefCell},
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
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
            event::listen_with(|event, status, _| match event {
                iced::Event::Keyboard(_) if status == event::Status::Ignored => {
                    Some(Message::Event(event))
                }
                _ => None,
            }),
            Subscription::run(pty_bridge),
            Subscription::run(transfer_bridge),
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

#[derive(Clone, Debug)]
enum TransferEvent {
    Started {
        id: u64,
        total: Option<u64>,
    },
    Progress {
        id: u64,
        transferred: u64,
    },
    Completed {
        id: u64,
    },
    Failed {
        id: u64,
        error: String,
        partial: Option<PathBuf>,
        resume_available: bool,
    },
    Cancelled {
        id: u64,
        partial: Option<PathBuf>,
        resume_available: bool,
    },
}

type TransferBridgeSender = mpsc::UnboundedSender<TransferEvent>;

fn transfer_bridge() -> impl Stream<Item = Message> {
    iced::stream::channel(100, async |mut output| {
        let (sender, mut receiver) = mpsc::unbounded();
        if output
            .send(Message::TransferBridgeReady(sender))
            .await
            .is_err()
        {
            return;
        }
        while let Some(event) = receiver.next().await {
            if output.send(Message::TransferEvent(event)).await.is_err() {
                break;
            }
        }
    })
}

fn send_transfer_event(sender: &TransferBridgeSender, event: TransferEvent) {
    let _ = sender.unbounded_send(event);
}

fn run_transfer_worker(
    id: u64,
    pending: PendingTransfer,
    overwrite: bool,
    resume_path: Option<PathBuf>,
    resume_requested: bool,
    cancel: Arc<AtomicBool>,
    config: Option<PathBuf>,
    sender: TransferBridgeSender,
) {
    let start = match (pending.direction, resume_requested) {
        (TransferDirection::Download, true) => {
            let Some(partial) = resume_path.as_deref() else {
                send_transfer_event(
                    &sender,
                    TransferEvent::Failed {
                        id,
                        error: "resumable download partial is missing".into(),
                        partial: None,
                        resume_available: false,
                    },
                );
                return;
            };
            sftp::start_download_resume(
                &pending.session,
                config.as_deref(),
                &pending.remote,
                &pending.local,
                partial,
                overwrite,
                pending.expected_size,
            )
        }
        (TransferDirection::Download, false) => sftp::start_download(
            &pending.session,
            config.as_deref(),
            &pending.remote,
            &pending.local,
            overwrite,
            pending.expected_size,
        ),
        (TransferDirection::Upload, true) => sftp::start_upload_resume(
            &pending.session,
            config.as_deref(),
            &pending.local,
            &pending.remote,
        ),
        (TransferDirection::Upload, false) => sftp::start_upload(
            &pending.session,
            config.as_deref(),
            &pending.local,
            &pending.remote,
        ),
    };

    let mut transfer = match start {
        Ok(transfer) => transfer,
        Err(error) => {
            send_transfer_event(
                &sender,
                TransferEvent::Failed {
                    id,
                    error: format!("{error:#}"),
                    partial: resume_path,
                    resume_available: false,
                },
            );
            return;
        }
    };

    send_transfer_event(
        &sender,
        TransferEvent::Started {
            id,
            total: transfer.expected_bytes(),
        },
    );

    let mut last_progress = u64::MAX;
    loop {
        if cancel.load(Ordering::Acquire) {
            match transfer.cancel() {
                Ok(partial) => send_transfer_event(
                    &sender,
                    TransferEvent::Cancelled {
                        id,
                        resume_available: pending.direction == TransferDirection::Upload
                            || partial.is_some(),
                        partial,
                    },
                ),
                Err(error) => send_transfer_event(
                    &sender,
                    TransferEvent::Failed {
                        id,
                        error: format!("cancel transfer: {error:#}"),
                        partial: None,
                        resume_available: pending.direction == TransferDirection::Upload,
                    },
                ),
            }
            return;
        }

        let transferred = transfer.transferred_bytes();
        if transferred != last_progress {
            last_progress = transferred;
            send_transfer_event(&sender, TransferEvent::Progress { id, transferred });
        }

        match transfer.poll() {
            Ok(Some(())) => {
                send_transfer_event(&sender, TransferEvent::Completed { id });
                return;
            }
            Ok(None) => thread::sleep(Duration::from_millis(200)),
            Err(error) => {
                let mut message = format!("{error:#}");
                let partial = match transfer.preserve_partial() {
                    Ok(partial) => partial,
                    Err(preserve_error) => {
                        message.push_str(&format!(
                            "; preserving resumable partial also failed: {preserve_error:#}"
                        ));
                        None
                    }
                };
                send_transfer_event(
                    &sender,
                    TransferEvent::Failed {
                        id,
                        error: message,
                        resume_available: pending.direction == TransferDirection::Upload
                            || partial.is_some(),
                        partial,
                    },
                );
                return;
            }
        }
    }
}

struct TerminalPane {
    id: u64,
    profile: Session,
    terminal: Option<egui_term::TerminalBackend>,
    display: Option<egui_term::DisplaySnapshot>,
    display_generation: u64,
    display_dirty: bool,
    terminal_grid_size: Option<(u16, u16)>,
    terminal_title: Option<String>,
    error: Option<String>,
    exited: bool,
    refresh_pending: Arc<AtomicBool>,
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TransferDirection {
    Upload,
    Download,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TransferJobState {
    Queued,
    Running,
    Completed,
    Cancelled,
    Failed,
}

#[derive(Clone, Debug)]
struct PendingTransfer {
    session: Session,
    session_key: String,
    direction: TransferDirection,
    local: PathBuf,
    remote: String,
    expected_size: Option<u64>,
}

#[derive(Clone, Debug)]
struct PreparedTransfer {
    pending: PendingTransfer,
    conflict: bool,
}

#[derive(Clone)]
struct RemoteEditHandle(Arc<Mutex<remote_edit::RemoteEdit>>);

impl std::fmt::Debug for RemoteEditHandle {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_tuple("RemoteEditHandle").finish()
    }
}

struct RemoteEditorState {
    session: Session,
    session_key: String,
    remote: String,
    handle: RemoteEditHandle,
    content: text_editor::Content,
    saving: bool,
    conflict: bool,
    discard_confirm: bool,
    error: Option<String>,
}

#[derive(Debug)]
struct TransferJob {
    id: u64,
    pending: PendingTransfer,
    overwrite: bool,
    total: Option<u64>,
    transferred: u64,
    state: TransferJobState,
    error: String,
    resume_path: Option<PathBuf>,
    resume_available: bool,
    resume_requested: bool,
    cancel: Arc<AtomicBool>,
}

impl TransferJob {
    fn label(&self) -> String {
        match self.pending.direction {
            TransferDirection::Upload => {
                let name = self
                    .pending
                    .local
                    .file_name()
                    .map(|value| value.to_string_lossy().into_owned())
                    .unwrap_or_else(|| self.pending.local.to_string_lossy().into_owned());
                format!("Upload {name}")
            }
            TransferDirection::Download => format!("Download {}", self.pending.remote),
        }
    }
}

#[derive(Clone, Debug)]
struct LocalFileEntry {
    path: PathBuf,
    name: String,
    is_dir: bool,
    size: Option<u64>,
}

struct FilesState {
    session_key: Option<String>,
    local_dir: PathBuf,
    remote_dir: String,
    local_entries: Vec<LocalFileEntry>,
    remote_entries: Vec<sftp::RemoteEntry>,
    local_loading: bool,
    remote_loading: bool,
    local_error: Option<String>,
    remote_error: Option<String>,
    local_generation: u64,
    remote_generation: u64,
    selected_local: Option<PathBuf>,
    selected_remote: Option<sftp::RemoteEntry>,
    name_input: String,
    transfers: Vec<TransferJob>,
}

impl FilesState {
    fn new() -> Self {
        Self {
            session_key: None,
            local_dir: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            remote_dir: ".".into(),
            local_entries: Vec::new(),
            remote_entries: Vec::new(),
            local_loading: false,
            remote_loading: false,
            local_error: None,
            remote_error: None,
            local_generation: 0,
            remote_generation: 0,
            selected_local: None,
            selected_remote: None,
            name_input: String::new(),
            transfers: Vec::new(),
        }
    }
}

fn remote_parent(path: &str) -> String {
    let trimmed = path.trim_end_matches('/');
    if trimmed.is_empty() || trimmed == "." || trimmed == "/" {
        return if trimmed == "/" {
            "/".into()
        } else {
            ".".into()
        };
    }
    match trimmed.rsplit_once('/') {
        Some(("", _)) => "/".into(),
        Some((parent, _)) if !parent.is_empty() => parent.into(),
        _ => ".".into(),
    }
}

fn display_leaf(value: &str) -> String {
    value
        .chars()
        .map(|ch| if ch.is_control() { '�' } else { ch })
        .collect()
}

fn format_file_size(size: Option<u64>) -> String {
    let Some(size) = size else {
        return String::new();
    };
    if size >= 1_073_741_824 {
        format!("{:.1} GiB", size as f64 / 1_073_741_824.0)
    } else if size >= 1_048_576 {
        format!("{:.1} MiB", size as f64 / 1_048_576.0)
    } else if size >= 1024 {
        format!("{:.1} KiB", size as f64 / 1024.0)
    } else {
        format!("{size} B")
    }
}

#[derive(Clone, Copy)]
enum Dock {
    Terminal,
    Files,
}

#[derive(Clone)]
enum FileNameAction {
    MkdirLocal(PathBuf),
    MkdirRemote {
        session: Session,
        session_key: String,
        directory: String,
    },
    RenameLocal(PathBuf),
    RenameRemote {
        session: Session,
        session_key: String,
        from: String,
    },
}

#[derive(Clone)]
enum Dialog {
    Connection,
    Commands,
    Close(usize),
    PasteConfirm {
        id: u64,
        text: String,
    },
    FileOverwrite(PendingTransfer),
    FileDeleteLocal(PathBuf),
    FileDeleteRemote {
        session: Session,
        session_key: String,
        path: String,
        directory: bool,
    },
    FileName(FileNameAction),
    ImportProfiles,
    About,
}

#[derive(Clone, Debug)]
enum Message {
    Search(String),
    Open(usize),
    EditProfile(usize),
    SelectTab(usize),
    SelectNextTab,
    SelectPreviousTab,
    New,
    ImportProfiles,
    ImportPath(String),
    ConfirmImportProfiles,
    Commands,
    CommandQuery(String),
    CommandSender(String),
    CommandStage(usize),
    CommandSend,
    SnippetName(String),
    SnippetBody(String),
    SaveSnippet,
    DeleteSnippet(usize),
    About,
    CloseDialog,
    AskClose(usize),
    ConfirmClose(usize),
    ToggleFiles,
    FilesRefresh,
    FilesLocalUp,
    FilesRemoteUp,
    FilesOpenLocal(PathBuf),
    FilesOpenRemote(String),
    FilesSelectLocal(PathBuf),
    FilesSelectRemote(sftp::RemoteEntry),
    FilesRequestUpload,
    FilesRequestDownload,
    FilesRequestDeleteLocal,
    FilesRequestDeleteRemote,
    FilesRequestMkdirLocal,
    FilesRequestMkdirRemote,
    FilesRequestRenameLocal,
    FilesRequestRenameRemote,
    FilesRequestEditRemote,
    RemoteEditorOpened {
        session: Session,
        session_key: String,
        remote: String,
        result: Result<RemoteEditHandle, String>,
    },
    RemoteEditorAction(text_editor::Action),
    RemoteEditorSave(bool),
    RemoteEditorSaved(Result<SaveOutcome, String>),
    RemoteEditorClose,
    RemoteEditorDiscard,
    RemoteEditorKeepEditing,
    RemoteEditorKeepConflict,
    FilesNameChanged(String),
    ConfirmFileNameAction,
    FilesMutationFinished {
        remote: bool,
        session_key: Option<String>,
        result: Result<String, String>,
    },
    FilesTransferPrepared(Result<PreparedTransfer, String>),
    ConfirmFileOverwrite,
    ConfirmFileDelete,
    FilesDeleteFinished {
        remote: bool,
        session_key: Option<String>,
        result: Result<String, String>,
    },
    CancelTransfer(u64),
    RetryTransfer(u64),
    ResumeTransfer(u64),
    FilesLocalLoaded(u64, Result<Vec<LocalFileEntry>, String>),
    FilesRemoteLoaded(u64, Result<Vec<sftp::RemoteEntry>, String>),
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
    Favorite(bool),
    TagInput(String),
    AddTag,
    RemoveTag(usize),
    StrictHostKey(bool),
    IdentityFile(String),
    ProxyJump(String),
    ProxyKind(ProxyKind),
    ProxyHost(String),
    ProxyPort(String),
    ProxyAuth(ProxyAuth),
    Ciphers(String),
    Macs(String),
    KexAlgorithms(String),
    HostKeyAlgorithms(String),
    ConnectTimeout(String),
    Keepalive(String),
    RemoteCommand(String),
    Advanced,
    Submit,
    Scale(f64),
    PtyBridgeReady(PtyBridgeSender),
    PtyEvent(u64, egui_term::PtyEvent),
    TerminalFrame,
    TransferBridgeReady(TransferBridgeSender),
    TransferEvent(TransferEvent),
    TerminalResized(u64, iced::Size),
    TerminalSelectStart(pane_grid::Pane, u64, f32, f32),
    TerminalSelectUpdate(u64, f32, f32),
    TerminalMouse(
        pane_grid::Pane,
        u64,
        egui_term::MouseButton,
        egui_term::MouseModifiers,
        f32,
        f32,
        bool,
    ),
    TerminalMouseWheel(
        pane_grid::Pane,
        u64,
        egui_term::MouseModifiers,
        f32,
        f32,
        i32,
    ),
    TerminalScroll(u64, i32),
    CopySelection(u64),
    Event(iced::Event),
}

fn iced_palette_items(query: &str, library: &SnippetLibrary) -> Vec<PaletteItem> {
    command_palette::palette_items(query, library)
        .into_iter()
        .filter(|item| {
            matches!(
                item,
                PaletteItem::Snippet { .. }
                    | PaletteItem::LocalAction { id: "connect", .. }
                    | PaletteItem::LocalAction {
                        id: "close-active",
                        ..
                    }
            )
        })
        .collect()
}

fn sanitize_terminal_title(title: &str) -> Option<String> {
    let value: String = title
        .chars()
        .filter(|ch| !ch.is_control())
        .take(120)
        .collect();
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_owned())
}

struct TerminalCanvasState {
    selecting: bool,
    remote_button: Option<egui_term::MouseButton>,
    modifiers: keyboard::Modifiers,
    last_position: Option<iced::Point>,
    last_report_cell: Option<(i32, i32)>,
    generation: Cell<u64>,
    row_hashes: RefCell<Vec<u64>>,
    row_caches: RefCell<Vec<canvas::Cache>>,
    background_cache: canvas::Cache,
}

impl Default for TerminalCanvasState {
    fn default() -> Self {
        Self {
            selecting: false,
            remote_button: None,
            modifiers: keyboard::Modifiers::default(),
            last_position: None,
            last_report_cell: None,
            generation: Cell::new(u64::MAX),
            row_hashes: RefCell::new(Vec::new()),
            row_caches: RefCell::new(Vec::new()),
            background_cache: canvas::Cache::new(),
        }
    }
}

impl TerminalCanvasState {
    /// Return true only when mouse motion crosses into a different terminal cell.
    /// This avoids one UI update per pixel during selection and mouse tracking.
    fn mark_cell_changed(&mut self, cell: (i32, i32)) -> bool {
        if self.last_report_cell == Some(cell) {
            return false;
        }
        self.last_report_cell = Some(cell);
        true
    }
}

struct TerminalCanvas<'a> {
    pane: pane_grid::Pane,
    id: u64,
    generation: u64,
    terminal_mode: egui_term::TerminalMode,
    snapshot: &'a egui_term::DisplaySnapshot,
}

impl TerminalCanvas<'_> {
    fn remote_mouse_enabled(&self, modifiers: keyboard::Modifiers) -> bool {
        self.terminal_mode
            .intersects(egui_term::TerminalMode::MOUSE_MODE)
            && !modifiers.shift()
    }

    fn mouse_modifiers(modifiers: keyboard::Modifiers) -> egui_term::MouseModifiers {
        egui_term::MouseModifiers {
            shift: modifiers.shift(),
            alt: modifiers.alt(),
            command: modifiers.command(),
        }
    }

    fn mouse_button(button: mouse::Button) -> Option<egui_term::MouseButton> {
        match button {
            mouse::Button::Left => Some(egui_term::MouseButton::LeftButton),
            mouse::Button::Middle => Some(egui_term::MouseButton::MiddleButton),
            mouse::Button::Right => Some(egui_term::MouseButton::RightButton),
            _ => None,
        }
    }

    fn movement_button(button: egui_term::MouseButton) -> egui_term::MouseButton {
        match button {
            egui_term::MouseButton::LeftButton => egui_term::MouseButton::LeftMove,
            egui_term::MouseButton::MiddleButton => egui_term::MouseButton::MiddleMove,
            egui_term::MouseButton::RightButton => egui_term::MouseButton::RightMove,
            _ => egui_term::MouseButton::NoneMove,
        }
    }

    fn report_cell(position: iced::Point) -> (i32, i32) {
        (
            (position.x / TERMINAL_CELL_WIDTH).floor() as i32,
            (position.y / TERMINAL_CELL_HEIGHT).floor() as i32,
        )
    }
}

impl canvas::Program<Message> for TerminalCanvas<'_> {
    type State = TerminalCanvasState;

    fn update(
        &self,
        state: &mut Self::State,
        event: &iced::Event,
        bounds: iced::Rectangle,
        cursor: mouse::Cursor,
    ) -> Option<canvas::Action<Message>> {
        let position = cursor.position_in(bounds);
        if let Some(position) = position {
            state.last_position = Some(position);
        }

        match event {
            iced::Event::Keyboard(keyboard::Event::ModifiersChanged(modifiers)) => {
                state.modifiers = *modifiers;
                None
            }
            iced::Event::Mouse(mouse::Event::ButtonPressed(button)) => {
                let position = position?;
                let terminal_button = Self::mouse_button(*button)?;
                if self.remote_mouse_enabled(state.modifiers) {
                    state.selecting = false;
                    state.remote_button = Some(terminal_button);
                    state.last_report_cell = Some(Self::report_cell(position));
                    Some(
                        canvas::Action::publish(Message::TerminalMouse(
                            self.pane,
                            self.id,
                            terminal_button,
                            Self::mouse_modifiers(state.modifiers),
                            position.x,
                            position.y,
                            true,
                        ))
                        .and_capture(),
                    )
                } else if *button == mouse::Button::Left {
                    state.selecting = true;
                    state.remote_button = None;
                    state.last_report_cell = Some(Self::report_cell(position));
                    Some(
                        canvas::Action::publish(Message::TerminalSelectStart(
                            self.pane, self.id, position.x, position.y,
                        ))
                        .and_capture(),
                    )
                } else {
                    None
                }
            }
            iced::Event::Mouse(mouse::Event::CursorMoved { .. }) => {
                let position = position?;
                if let Some(button) = state.remote_button {
                    if self.remote_mouse_enabled(state.modifiers)
                        && self.terminal_mode.intersects(
                            egui_term::TerminalMode::MOUSE_DRAG
                                | egui_term::TerminalMode::MOUSE_MOTION,
                        )
                    {
                        if !state.mark_cell_changed(Self::report_cell(position)) {
                            return None;
                        }
                        return Some(
                            canvas::Action::publish(Message::TerminalMouse(
                                self.pane,
                                self.id,
                                Self::movement_button(button),
                                Self::mouse_modifiers(state.modifiers),
                                position.x,
                                position.y,
                                true,
                            ))
                            .and_capture(),
                        );
                    }
                    None
                } else if state.selecting {
                    // Selection boundaries follow terminal cells, not individual pixels.
                    // Coalesce intra-cell mouse moves so a fast drag cannot flood the
                    // UI loop with full terminal-selection snapshots.
                    if !state.mark_cell_changed(Self::report_cell(position)) {
                        return None;
                    }
                    Some(
                        canvas::Action::publish(Message::TerminalSelectUpdate(
                            self.id, position.x, position.y,
                        ))
                        .and_capture(),
                    )
                } else if self.remote_mouse_enabled(state.modifiers)
                    && self
                        .terminal_mode
                        .contains(egui_term::TerminalMode::MOUSE_MOTION)
                {
                    if !state.mark_cell_changed(Self::report_cell(position)) {
                        return None;
                    }
                    Some(
                        canvas::Action::publish(Message::TerminalMouse(
                            self.pane,
                            self.id,
                            egui_term::MouseButton::NoneMove,
                            Self::mouse_modifiers(state.modifiers),
                            position.x,
                            position.y,
                            true,
                        ))
                        .and_capture(),
                    )
                } else {
                    None
                }
            }
            iced::Event::Mouse(mouse::Event::ButtonReleased(button)) => {
                let terminal_button = Self::mouse_button(*button)?;
                if state.remote_button == Some(terminal_button) {
                    state.remote_button = None;
                    let position = position.or(state.last_position)?;
                    Some(
                        canvas::Action::publish(Message::TerminalMouse(
                            self.pane,
                            self.id,
                            terminal_button,
                            Self::mouse_modifiers(state.modifiers),
                            position.x,
                            position.y,
                            false,
                        ))
                        .and_capture(),
                    )
                } else if state.selecting && *button == mouse::Button::Left {
                    state.selecting = false;
                    position.or(state.last_position).map(|position| {
                        canvas::Action::publish(Message::TerminalSelectUpdate(
                            self.id, position.x, position.y,
                        ))
                        .and_capture()
                    })
                } else {
                    None
                }
            }
            iced::Event::Mouse(mouse::Event::WheelScrolled { delta }) => {
                let lines = match delta {
                    mouse::ScrollDelta::Lines { y, .. } => *y,
                    mouse::ScrollDelta::Pixels { y, .. } => *y / TERMINAL_CELL_HEIGHT,
                };
                let lines = lines.round() as i32;
                if lines == 0 {
                    return None;
                }
                if self.remote_mouse_enabled(state.modifiers) {
                    let position = position.or(state.last_position)?;
                    Some(
                        canvas::Action::publish(Message::TerminalMouseWheel(
                            self.pane,
                            self.id,
                            Self::mouse_modifiers(state.modifiers),
                            position.x,
                            position.y,
                            lines,
                        ))
                        .and_capture(),
                    )
                } else {
                    Some(
                        canvas::Action::publish(Message::TerminalScroll(self.id, lines))
                            .and_capture(),
                    )
                }
            }
            _ => None,
        }
    }

    fn mouse_interaction(
        &self,
        state: &Self::State,
        bounds: iced::Rectangle,
        cursor: mouse::Cursor,
    ) -> mouse::Interaction {
        if cursor.position_in(bounds).is_some() {
            if state.selecting {
                mouse::Interaction::Grabbing
            } else if self.remote_mouse_enabled(state.modifiers) {
                mouse::Interaction::Pointer
            } else {
                mouse::Interaction::Text
            }
        } else {
            mouse::Interaction::default()
        }
    }

    fn draw(
        &self,
        state: &Self::State,
        renderer: &iced::Renderer,
        _theme: &Theme,
        bounds: iced::Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<canvas::Geometry> {
        fn rgb(value: [u8; 3]) -> Color {
            Color::from_rgb8(value[0], value[1], value[2])
        }

        if state.generation.get() != self.generation {
            use std::{
                collections::hash_map::DefaultHasher,
                hash::{Hash, Hasher},
            };

            let mut new_hashes = Vec::with_capacity(self.snapshot.rows);
            for row in 0..self.snapshot.rows {
                let mut hasher = DefaultHasher::new();
                self.snapshot.background.hash(&mut hasher);
                self.snapshot.columns.hash(&mut hasher);
                row.hash(&mut hasher);
                if let Some(&(start, end)) = self.snapshot.row_ranges.get(row) {
                    self.snapshot.cells[start..end].hash(&mut hasher);
                }
                new_hashes.push(hasher.finish());
            }

            let mut hashes = state.row_hashes.borrow_mut();
            let mut caches = state.row_caches.borrow_mut();
            hashes.resize(self.snapshot.rows, u64::MAX);
            caches.resize_with(self.snapshot.rows, canvas::Cache::new);
            for row in 0..self.snapshot.rows {
                if hashes[row] != new_hashes[row] {
                    caches[row].clear();
                    hashes[row] = new_hashes[row];
                }
            }
            state.background_cache.clear();
            state.generation.set(self.generation);
        }

        let background_geometry = state
            .background_cache
            .draw(renderer, bounds.size(), |frame| {
                frame.fill(
                    &canvas::Path::rectangle(iced::Point::ORIGIN, bounds.size()),
                    rgb(self.snapshot.background),
                );
            });

        let caches = state.row_caches.borrow();
        let mut geometries = Vec::with_capacity(self.snapshot.rows + 1);
        geometries.push(background_geometry);

        for row in 0..self.snapshot.rows {
            let Some(&(start, end)) = self.snapshot.row_ranges.get(row) else {
                continue;
            };
            let geometry = caches[row].draw(renderer, bounds.size(), |frame| {
                for cell in &self.snapshot.cells[start..end] {
                    let x = cell.column as f32 * TERMINAL_CELL_WIDTH;
                    let y = cell.row as f32 * TERMINAL_CELL_HEIGHT;
                    if x >= bounds.width || y >= bounds.height {
                        continue;
                    }

                    let cell_width = if cell.wide {
                        TERMINAL_CELL_WIDTH * 2.0
                    } else {
                        TERMINAL_CELL_WIDTH
                    };
                    let background = if cell.cursor {
                        cell.cursor_color
                    } else {
                        cell.background
                    };
                    if background != self.snapshot.background || cell.cursor {
                        frame.fill(
                            &canvas::Path::rectangle(
                                iced::Point::new(x, y),
                                iced::Size::new(cell_width + 0.5, TERMINAL_CELL_HEIGHT + 0.5),
                            ),
                            rgb(background),
                        );
                    }

                    if !matches!(cell.character, ' ' | '\t' | '\0') {
                        let mut terminal_font = Font::MONOSPACE;
                        if cell.bold {
                            terminal_font.weight = font::Weight::Bold;
                        }
                        if cell.italic {
                            terminal_font.style = font::Style::Italic;
                        }
                        let foreground = if cell.cursor {
                            cell.background
                        } else {
                            cell.foreground
                        };
                        frame.fill_text(canvas::Text {
                            content: cell.character.to_string(),
                            position: iced::Point::new(x, y - 1.0),
                            color: rgb(foreground),
                            size: iced::Pixels(15.0),
                            font: terminal_font,
                            ..canvas::Text::default()
                        });
                    }

                    if cell.underline {
                        frame.fill(
                            &canvas::Path::rectangle(
                                iced::Point::new(x, y + TERMINAL_CELL_HEIGHT - 2.0),
                                iced::Size::new(cell_width, 1.0),
                            ),
                            rgb(cell.foreground),
                        );
                    }
                    if cell.strikeout {
                        frame.fill(
                            &canvas::Path::rectangle(
                                iced::Point::new(x, y + TERMINAL_CELL_HEIGHT * 0.55),
                                iced::Size::new(cell_width, 1.0),
                            ),
                            rgb(cell.foreground),
                        );
                    }
                }
            });
            geometries.push(geometry);
        }

        geometries
    }
}

fn terminal_key_bytes(
    key: &keyboard::Key,
    modifiers: keyboard::Modifiers,
    committed_text: Option<&str>,
    terminal_mode: egui_term::TerminalMode,
) -> Option<Vec<u8>> {
    use keyboard::key::Named;

    if modifiers.control() {
        if let keyboard::Key::Character(value) = key.as_ref() {
            let mut chars = value.chars();
            if let (Some(ch), None) = (chars.next(), chars.next()) {
                let ch = ch.to_ascii_lowercase();
                let control = match ch {
                    'a'..='z' => Some((ch as u8 - b'a') + 1),
                    ' ' | '@' => Some(0),
                    '[' => Some(0x1b),
                    '\\' => Some(0x1c),
                    ']' => Some(0x1d),
                    '^' => Some(0x1e),
                    '_' => Some(0x1f),
                    '?' => Some(0x7f),
                    _ => None,
                };
                if let Some(control) = control {
                    return Some(vec![control]);
                }
            }
        }
    }

    let app_cursor = terminal_mode.contains(egui_term::TerminalMode::APP_CURSOR);
    let cursor = |normal: &'static [u8], application: &'static [u8]| {
        Some(if app_cursor { application } else { normal }.to_vec())
    };
    let xterm_modifier = 1
        + u8::from(modifiers.shift())
        + 2 * u8::from(modifiers.alt())
        + 4 * u8::from(modifiers.control());
    let modified_cursor = |suffix: u8| {
        (xterm_modifier > 1)
            .then(|| format!("\x1b[1;{}{}", xterm_modifier, suffix as char).into_bytes())
    };

    match key.as_ref() {
        keyboard::Key::Named(Named::Enter) => Some(vec![b'\r']),
        keyboard::Key::Named(Named::Tab) if modifiers.shift() => Some(b"\x1b[Z".to_vec()),
        keyboard::Key::Named(Named::Tab) => Some(vec![b'\t']),
        keyboard::Key::Named(Named::Escape) => Some(vec![0x1b]),
        keyboard::Key::Named(Named::Backspace) => Some(vec![0x7f]),
        keyboard::Key::Named(Named::Insert) => Some(b"\x1b[2~".to_vec()),
        keyboard::Key::Named(Named::Delete) => Some(b"\x1b[3~".to_vec()),
        keyboard::Key::Named(Named::ArrowUp) => {
            modified_cursor(b'A').or_else(|| cursor(b"\x1b[A", b"\x1bOA"))
        }
        keyboard::Key::Named(Named::ArrowDown) => {
            modified_cursor(b'B').or_else(|| cursor(b"\x1b[B", b"\x1bOB"))
        }
        keyboard::Key::Named(Named::ArrowRight) => {
            modified_cursor(b'C').or_else(|| cursor(b"\x1b[C", b"\x1bOC"))
        }
        keyboard::Key::Named(Named::ArrowLeft) => {
            modified_cursor(b'D').or_else(|| cursor(b"\x1b[D", b"\x1bOD"))
        }
        keyboard::Key::Named(Named::Home) => {
            modified_cursor(b'H').or_else(|| cursor(b"\x1b[H", b"\x1bOH"))
        }
        keyboard::Key::Named(Named::End) => {
            modified_cursor(b'F').or_else(|| cursor(b"\x1b[F", b"\x1bOF"))
        }
        keyboard::Key::Named(Named::PageUp) => Some(b"\x1b[5~".to_vec()),
        keyboard::Key::Named(Named::PageDown) => Some(b"\x1b[6~".to_vec()),
        keyboard::Key::Named(Named::F1) => Some(b"\x1bOP".to_vec()),
        keyboard::Key::Named(Named::F2) => Some(b"\x1bOQ".to_vec()),
        keyboard::Key::Named(Named::F3) => Some(b"\x1bOR".to_vec()),
        keyboard::Key::Named(Named::F4) => Some(b"\x1bOS".to_vec()),
        keyboard::Key::Named(Named::F5) => Some(b"\x1b[15~".to_vec()),
        keyboard::Key::Named(Named::F6) => Some(b"\x1b[17~".to_vec()),
        keyboard::Key::Named(Named::F7) => Some(b"\x1b[18~".to_vec()),
        keyboard::Key::Named(Named::F8) => Some(b"\x1b[19~".to_vec()),
        keyboard::Key::Named(Named::F9) => Some(b"\x1b[20~".to_vec()),
        keyboard::Key::Named(Named::F10) => Some(b"\x1b[21~".to_vec()),
        keyboard::Key::Named(Named::F11) => Some(b"\x1b[23~".to_vec()),
        keyboard::Key::Named(Named::F12) => Some(b"\x1b[24~".to_vec()),
        _ if modifiers.alt() && !modifiers.control() && !modifiers.macos_command() => {
            committed_text.filter(|text| !text.is_empty()).map(|text| {
                let mut bytes = Vec::with_capacity(text.len() + 1);
                bytes.push(0x1b);
                bytes.extend_from_slice(text.as_bytes());
                bytes
            })
        }
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
    favorite: bool,
    tags: Vec<String>,
    tag_input: String,
    strict_host_key: bool,
    identity_file: String,
    proxy_jump: String,
    proxy_kind: ProxyKind,
    proxy_host: String,
    proxy_port: String,
    proxy_auth: ProxyAuth,
    ciphers: String,
    macs: String,
    kex_algorithms: String,
    host_key_algorithms: String,
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
            favorite: false,
            tags: Vec::new(),
            tag_input: String::new(),
            strict_host_key: false,
            identity_file: String::new(),
            proxy_jump: String::new(),
            proxy_kind: ProxyKind::None,
            proxy_host: String::new(),
            proxy_port: String::new(),
            proxy_auth: ProxyAuth::None,
            ciphers: String::new(),
            macs: String::new(),
            kex_algorithms: String::new(),
            host_key_algorithms: String::new(),
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
            favorite: session.favorite,
            tags: session.tags.clone(),
            tag_input: String::new(),
            strict_host_key: session.strict,
            identity_file: session.ssh.identity_file.clone(),
            proxy_jump: session.ssh.proxy_jump.clone(),
            proxy_kind: session.ssh.proxy_kind,
            proxy_host: session.ssh.proxy_host.clone(),
            proxy_port: session
                .ssh
                .proxy_port
                .map(|value| value.to_string())
                .unwrap_or_default(),
            proxy_auth: session.ssh.proxy_auth,
            ciphers: session.ssh.ciphers.clone(),
            macs: session.ssh.macs.clone(),
            kex_algorithms: session.ssh.kex_algorithms.clone(),
            host_key_algorithms: session.ssh.host_key_algorithms.clone(),
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
            advanced: session.strict
                || !session.ssh.identity_file.is_empty()
                || !session.ssh.proxy_jump.is_empty()
                || session.ssh.proxy_kind != ProxyKind::None
                || !session.ssh.ciphers.is_empty()
                || !session.ssh.macs.is_empty()
                || !session.ssh.kex_algorithms.is_empty()
                || !session.ssh.host_key_algorithms.is_empty()
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
        session.favorite = self.favorite;
        session.tags = self.tags.clone();
        session.strict = self.strict_host_key;

        session.ssh.identity_file = self.identity_file.trim().to_owned();
        session.ssh.proxy_jump = self.proxy_jump.trim().to_owned();
        session.ssh.proxy_kind = self.proxy_kind;
        if self.proxy_kind == ProxyKind::None {
            session.ssh.proxy_host.clear();
            session.ssh.proxy_port = None;
            session.ssh.proxy_auth = ProxyAuth::None;
        } else {
            session.ssh.proxy_host = self.proxy_host.trim().to_owned();
            session.ssh.proxy_port = Self::optional_positive_u16("Proxy port", &self.proxy_port)?;
            session.ssh.proxy_auth = self.proxy_auth;
        }
        session.ssh.ciphers = self.ciphers.trim().to_owned();
        session.ssh.macs = self.macs.trim().to_owned();
        session.ssh.kex_algorithms = self.kex_algorithms.trim().to_owned();
        session.ssh.host_key_algorithms = self.host_key_algorithms.trim().to_owned();
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
    files: FilesState,
    remote_editor: Option<RemoteEditorState>,
    snippets_path: PathBuf,
    snippets: SnippetLibrary,
    command_query: String,
    command_sender: String,
    snippet_name: String,
    snippet_body: String,
    sidebar_collapsed: bool,
    form: ConnectionForm,
    import_path: String,
    editing_profile: Option<String>,
    dialog: Option<Dialog>,
    scale: f64,
    status: String,
    load_error: Option<String>,
    paste_policy: PastePolicy,
    pty_bridge: Option<PtyBridgeSender>,
    terminal_frame_scheduled: bool,
    transfer_bridge: Option<TransferBridgeSender>,
    next_terminal_id: u64,
    next_transfer_id: u64,
}

impl App {
    fn boot(profiles_path: PathBuf, ssh_config: Option<PathBuf>) -> Self {
        let (dock, terminal_dock) = pane_grid::State::new(Dock::Terminal);
        let (profiles, mut load_error) = match load_sessions(&profiles_path) {
            Ok(profiles) => (profiles, None),
            Err(error) => (
                Vec::new(),
                Some(format!("Could not load profiles: {error:#}")),
            ),
        };
        let snippets_path = command_palette::snippets_path(&profiles_path);
        let snippets = match command_palette::load_library(&snippets_path) {
            Ok(library) => library,
            Err(error) => {
                let message = format!("Could not load snippets: {error:#}");
                load_error = Some(match load_error {
                    Some(existing) => format!("{existing}\n{message}"),
                    None => message,
                });
                SnippetLibrary::default()
            }
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
            files: FilesState::new(),
            remote_editor: None,
            snippets_path,
            snippets,
            command_query: String::new(),
            command_sender: String::new(),
            snippet_name: String::new(),
            snippet_body: String::new(),
            sidebar_collapsed: false,
            form: ConnectionForm::default(),
            import_path: String::new(),
            editing_profile: None,
            dialog: None,
            scale: 1.0,
            status: "Ready".into(),
            load_error,
            paste_policy: PastePolicy::ConfirmMultiline,
            pty_bridge: None,
            terminal_frame_scheduled: false,
            transfer_bridge: None,
            next_terminal_id: 1,
            next_transfer_id: 1,
        }
    }

    fn new_terminal_pane(&mut self, profile: Session) -> TerminalPane {
        let id = self.next_terminal_id;
        self.next_terminal_id = self.next_terminal_id.saturating_add(1);

        let refresh_pending = Arc::new(AtomicBool::new(false));
        let mut pane = TerminalPane {
            id,
            profile: profile.clone(),
            terminal: None,
            display: None,
            display_generation: 0,
            display_dirty: true,
            terminal_grid_size: None,
            terminal_title: None,
            error: None,
            exited: false,
            refresh_pending: refresh_pending.clone(),
        };

        let Some(bridge) = self.pty_bridge.clone() else {
            pane.error = Some(
                "Terminal event bridge is still initializing. Close and reopen this session."
                    .into(),
            );
            return pane;
        };

        let bridge = Arc::new(Mutex::new(bridge));
        let event_refresh_pending = refresh_pending.clone();
        let event_sink: Arc<dyn Fn(u64, egui_term::PtyEvent) + Send + Sync> =
            Arc::new(move |id, event| {
                let is_wakeup = matches!(&event, egui_term::PtyEvent::Wakeup);
                if is_wakeup && event_refresh_pending.swap(true, Ordering::AcqRel) {
                    return;
                }

                let sent = bridge
                    .lock()
                    .is_ok_and(|sender| sender.unbounded_send((id, event)).is_ok());
                if is_wakeup && !sent {
                    event_refresh_pending.store(false, Ordering::Release);
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

    fn focused_terminal_mode(&self) -> egui_term::TerminalMode {
        self.tabs
            .get(self.active)
            .and_then(|tab| tab.panes.get(tab.focus))
            .and_then(|pane| pane.terminal.as_ref())
            .map(|terminal| terminal.last_content().terminal_mode)
            .unwrap_or_else(egui_term::TerminalMode::empty)
    }

    fn send_to_terminal(&mut self, id: u64, bytes: Vec<u8>) -> bool {
        self.command_terminal(id, egui_term::BackendCommand::Write(bytes))
    }

    fn command_terminal(&mut self, id: u64, command: egui_term::BackendCommand) -> bool {
        let mut command = Some(command);
        for tab in &mut self.tabs {
            for (_, pane) in tab.panes.iter_mut() {
                if pane.id != id || pane.exited {
                    continue;
                }
                if let Some(terminal) = pane.terminal.as_mut() {
                    terminal.process_command(command.take().expect("terminal command"));
                    return true;
                }
                return false;
            }
        }
        false
    }

    fn selected_terminal_text(&mut self, id: u64) -> Option<String> {
        for tab in &mut self.tabs {
            for (_, pane) in tab.panes.iter_mut() {
                if pane.id != id {
                    continue;
                }
                let terminal = pane.terminal.as_mut()?;
                terminal.sync();
                let selection = terminal.selectable_content();
                return (!selection.is_empty()).then_some(selection);
            }
        }
        None
    }

    fn refresh_terminal_display(&mut self, id: u64) {
        for tab in &mut self.tabs {
            for (_, pane) in tab.panes.iter_mut() {
                if pane.id != id {
                    continue;
                }
                if let Some(terminal) = pane.terminal.as_mut() {
                    let snapshot = terminal.display_snapshot(&egui_term::TerminalTheme::default());
                    if pane.display.as_ref() != Some(&snapshot) {
                        pane.display = Some(snapshot);
                        pane.display_generation = pane.display_generation.wrapping_add(1);
                    }
                    pane.display_dirty = false;
                }
                return;
            }
        }
    }

    fn queue_terminal_refresh(&mut self, id: u64, exited: bool) {
        for tab in &mut self.tabs {
            for (_, pane) in tab.panes.iter_mut() {
                if pane.id != id {
                    continue;
                }
                pane.display_dirty = true;
                if exited {
                    pane.exited = true;
                }
                return;
            }
        }
    }

    fn refresh_workspace_displays(&mut self, index: usize) {
        let Some(tab) = self.tabs.get_mut(index) else {
            return;
        };
        for (_, pane) in tab.panes.iter_mut() {
            if pane.display_dirty {
                if let Some(terminal) = pane.terminal.as_mut() {
                    let snapshot = terminal.display_snapshot(&egui_term::TerminalTheme::default());
                    if pane.display.as_ref() != Some(&snapshot) {
                        pane.display = Some(snapshot);
                        pane.display_generation = pane.display_generation.wrapping_add(1);
                    }
                }
                pane.display_dirty = false;
            }
            pane.refresh_pending.store(false, Ordering::Release);
        }
    }

    fn activate_tab(&mut self, index: usize) {
        if index < self.tabs.len() {
            self.active = index;
            self.refresh_workspace_displays(index);
        }
    }

    fn update_terminal_title(&mut self, id: u64, title: Option<String>) {
        for tab in &mut self.tabs {
            for (_, pane) in tab.panes.iter_mut() {
                if pane.id == id {
                    pane.terminal_title = title;
                    return;
                }
            }
        }
    }

    fn resize_terminal(&mut self, id: u64, size: iced::Size) {
        let width = (size.width - 28.0).max(1.0);
        let height = (size.height - 28.0).max(1.0);
        let columns = (width / TERMINAL_CELL_WIDTH).floor().max(1.0) as u16;
        let lines = (height / TERMINAL_CELL_HEIGHT).floor().max(1.0) as u16;
        let grid_size = (columns, lines);

        for tab in &mut self.tabs {
            for (_, pane) in tab.panes.iter_mut() {
                if pane.id != id {
                    continue;
                }
                if pane.terminal_grid_size == Some(grid_size) {
                    return;
                }
                pane.terminal_grid_size = Some(grid_size);
                if let Some(terminal) = pane.terminal.as_mut() {
                    terminal.process_command(egui_term::BackendCommand::Resize(
                        egui_term::Size::new(
                            columns as f32 * TERMINAL_CELL_WIDTH,
                            lines as f32 * TERMINAL_CELL_HEIGHT,
                        ),
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
                self.activate_tab(existing);
            } else {
                let terminal = self.new_terminal_pane(profile.clone());
                self.tabs.push(Workspace::new(profile, terminal));
                self.activate_tab(self.tabs.len() - 1);
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

    fn prepare_files_session(&mut self) {
        let Some(profile) = self.tabs.get(self.active).map(|tab| &tab.profile) else {
            self.files.session_key = None;
            return;
        };
        let key = session_profile_key(profile);
        if self.files.session_key.as_deref() != Some(key.as_str()) {
            self.files.session_key = Some(key);
            self.files.remote_dir = ".".into();
            self.files.remote_entries.clear();
            self.files.selected_remote = None;
            self.files.remote_error = None;
            self.files.remote_generation = self.files.remote_generation.wrapping_add(1);
        }
    }

    fn reload_local_files(&mut self) -> Task<Message> {
        self.files.local_loading = true;
        self.files.local_error = None;
        self.files.local_generation = self.files.local_generation.wrapping_add(1);
        let generation = self.files.local_generation;
        let directory = self.files.local_dir.clone();

        Task::perform(
            async move {
                sftp::local_entries(&directory)
                    .map(|paths| {
                        paths
                            .into_iter()
                            .map(|path| {
                                let metadata = std::fs::symlink_metadata(&path).ok();
                                let is_dir =
                                    metadata.as_ref().is_some_and(std::fs::Metadata::is_dir);
                                let size = metadata
                                    .as_ref()
                                    .filter(|metadata| metadata.is_file())
                                    .map(std::fs::Metadata::len);
                                let name = path
                                    .file_name()
                                    .map(|value| value.to_string_lossy().into_owned())
                                    .unwrap_or_else(|| path.to_string_lossy().into_owned());
                                LocalFileEntry {
                                    path,
                                    name,
                                    is_dir,
                                    size,
                                }
                            })
                            .collect::<Vec<_>>()
                    })
                    .map_err(|error| format!("{error:#}"))
            },
            move |result| Message::FilesLocalLoaded(generation, result),
        )
    }

    fn reload_remote_files(&mut self) -> Task<Message> {
        let Some(profile) = self.tabs.get(self.active).map(|tab| tab.profile.clone()) else {
            return Task::none();
        };
        self.prepare_files_session();
        self.files.remote_loading = true;
        self.files.remote_error = None;
        self.files.remote_generation = self.files.remote_generation.wrapping_add(1);
        let generation = self.files.remote_generation;
        let directory = self.files.remote_dir.clone();
        let config = self.ssh_config.clone();

        Task::perform(
            async move {
                sftp::list_remote(&profile, config.as_deref(), &directory)
                    .map_err(|error| format!("{error:#}"))
            },
            move |result| Message::FilesRemoteLoaded(generation, result),
        )
    }

    fn reload_files(&mut self) -> Task<Message> {
        self.prepare_files_session();
        Task::batch([self.reload_local_files(), self.reload_remote_files()])
    }

    fn request_upload(&mut self) -> Task<Message> {
        let Some(local) = self.files.selected_local.clone() else {
            self.status = "Select a local file to upload.".into();
            return Task::none();
        };
        let Some(profile) = self.tabs.get(self.active).map(|tab| tab.profile.clone()) else {
            return Task::none();
        };
        let session_key = session_profile_key(&profile);
        let remote_dir = self.files.remote_dir.clone();
        let remote_entries = self.files.remote_entries.clone();

        Task::perform(
            async move {
                let metadata = std::fs::metadata(&local)
                    .map_err(|error| format!("read upload source: {error}"))?;
                if !metadata.is_file() {
                    return Err("upload source must be a regular file".into());
                }
                let name = local
                    .file_name()
                    .and_then(|value| value.to_str())
                    .ok_or_else(|| "local filename must be valid Unicode".to_string())?
                    .to_owned();
                sftp::validate_local_leaf(&name).map_err(|error| format!("{error:#}"))?;
                let remote = sftp::join_remote(&remote_dir, &name);
                let conflict = remote_entries.iter().any(|entry| entry.name == name);
                Ok(PreparedTransfer {
                    pending: PendingTransfer {
                        session: profile,
                        session_key,
                        direction: TransferDirection::Upload,
                        local,
                        remote,
                        expected_size: Some(metadata.len()),
                    },
                    conflict,
                })
            },
            Message::FilesTransferPrepared,
        )
    }

    fn request_download(&mut self) -> Task<Message> {
        let Some(remote_entry) = self.files.selected_remote.clone() else {
            self.status = "Select a remote file to download.".into();
            return Task::none();
        };
        if remote_entry.is_dir {
            self.status = "Select a remote file, not a directory.".into();
            return Task::none();
        }
        let Some(profile) = self.tabs.get(self.active).map(|tab| tab.profile.clone()) else {
            return Task::none();
        };
        let session_key = session_profile_key(&profile);
        let local_dir = self.files.local_dir.clone();
        let remote = sftp::join_remote(&self.files.remote_dir, &remote_entry.name);

        Task::perform(
            async move {
                let local = sftp::local_destination(&local_dir, &remote_entry.name)
                    .map_err(|error| format!("{error:#}"))?;
                let conflict = local.exists();
                Ok(PreparedTransfer {
                    pending: PendingTransfer {
                        session: profile,
                        session_key,
                        direction: TransferDirection::Download,
                        local,
                        remote,
                        expected_size: remote_entry.size,
                    },
                    conflict,
                })
            },
            Message::FilesTransferPrepared,
        )
    }

    fn queue_transfer(&mut self, pending: PendingTransfer, overwrite: bool) {
        let id = self.next_transfer_id;
        self.next_transfer_id = self.next_transfer_id.saturating_add(1);
        let total = pending.expected_size;
        let label = match pending.direction {
            TransferDirection::Upload => "Upload",
            TransferDirection::Download => "Download",
        };
        self.files.transfers.push(TransferJob {
            id,
            pending,
            overwrite,
            total,
            transferred: 0,
            state: TransferJobState::Queued,
            error: String::new(),
            resume_path: None,
            resume_available: false,
            resume_requested: false,
            cancel: Arc::new(AtomicBool::new(false)),
        });
        self.status = format!("{label} queued.");
        self.try_start_next_transfer();
    }

    fn try_start_next_transfer(&mut self) {
        if self
            .files
            .transfers
            .iter()
            .any(|job| job.state == TransferJobState::Running)
        {
            return;
        }
        let Some(sender) = self.transfer_bridge.clone() else {
            return;
        };
        let Some(index) = self
            .files
            .transfers
            .iter()
            .position(|job| job.state == TransferJobState::Queued)
        else {
            return;
        };

        let (id, pending, overwrite, resume_path, resume_requested, cancel, config) = {
            let job = &mut self.files.transfers[index];
            job.state = TransferJobState::Running;
            job.error.clear();
            job.cancel.store(false, Ordering::Release);
            (
                job.id,
                job.pending.clone(),
                job.overwrite,
                job.resume_path.clone(),
                job.resume_requested,
                job.cancel.clone(),
                self.ssh_config.clone(),
            )
        };

        let spawn = thread::Builder::new()
            .name(format!("sftp-transfer-{id}"))
            .spawn(move || {
                run_transfer_worker(
                    id,
                    pending,
                    overwrite,
                    resume_path,
                    resume_requested,
                    cancel,
                    config,
                    sender,
                );
            });

        if let Err(error) = spawn {
            if let Some(job) = self.files.transfers.iter_mut().find(|job| job.id == id) {
                job.state = TransferJobState::Failed;
                job.error = format!("start transfer worker: {error}");
            }
            self.try_start_next_transfer();
        }
    }

    fn cancel_transfer(&mut self, id: u64) {
        if let Some(job) = self.files.transfers.iter_mut().find(|job| job.id == id) {
            match job.state {
                TransferJobState::Running => {
                    job.cancel.store(true, Ordering::Release);
                    self.status = format!("Cancelling {}...", job.label());
                }
                TransferJobState::Queued => {
                    job.state = TransferJobState::Cancelled;
                    self.status = format!("{} cancelled.", job.label());
                    self.try_start_next_transfer();
                }
                _ => {}
            }
        }
    }

    fn retry_transfer(&mut self, id: u64, resume: bool) {
        let Some(job) = self.files.transfers.iter_mut().find(|job| job.id == id) else {
            return;
        };
        if !matches!(
            job.state,
            TransferJobState::Failed | TransferJobState::Cancelled
        ) {
            return;
        }
        if resume && !job.resume_available {
            self.status = "This transfer has no resumable partial.".into();
            return;
        }
        job.resume_requested = resume;
        if !resume {
            job.resume_path = None;
        }
        job.transferred = 0;
        job.error.clear();
        job.state = TransferJobState::Queued;
        self.status = if resume {
            format!("{} queued for resume.", job.label())
        } else {
            format!("{} queued for retry.", job.label())
        };
        self.try_start_next_transfer();
    }

    fn handle_transfer_event(&mut self, event: TransferEvent) -> bool {
        let (id, terminal) = match &event {
            TransferEvent::Started { id, .. }
            | TransferEvent::Progress { id, .. }
            | TransferEvent::Completed { id }
            | TransferEvent::Failed { id, .. }
            | TransferEvent::Cancelled { id, .. } => (
                *id,
                matches!(
                    event,
                    TransferEvent::Completed { .. }
                        | TransferEvent::Failed { .. }
                        | TransferEvent::Cancelled { .. }
                ),
            ),
        };
        let Some(job) = self.files.transfers.iter_mut().find(|job| job.id == id) else {
            return false;
        };
        let active_session = self.files.session_key.clone();
        let refresh_active = terminal
            && active_session.as_deref() == Some(job.pending.session_key.as_str())
            && self.files_dock.is_some();

        match event {
            TransferEvent::Started { total, .. } => {
                job.total = total.or(job.total);
                job.state = TransferJobState::Running;
                self.status = format!("{} started.", job.label());
            }
            TransferEvent::Progress { transferred, .. } => {
                job.transferred = transferred;
            }
            TransferEvent::Completed { .. } => {
                job.transferred = job.total.unwrap_or(job.transferred);
                job.state = TransferJobState::Completed;
                job.error.clear();
                job.resume_path = None;
                job.resume_available = false;
                job.resume_requested = false;
                self.status = format!("{} completed and verified.", job.label());
            }
            TransferEvent::Failed {
                error,
                partial,
                resume_available,
                ..
            } => {
                job.state = TransferJobState::Failed;
                job.error = error;
                job.resume_path = partial;
                job.resume_available = resume_available;
                self.status = format!("{} failed.", job.label());
            }
            TransferEvent::Cancelled {
                partial,
                resume_available,
                ..
            } => {
                job.state = TransferJobState::Cancelled;
                job.resume_path = partial;
                job.resume_available = resume_available;
                self.status = format!("{} cancelled.", job.label());
            }
        }

        if terminal {
            self.try_start_next_transfer();
        }
        refresh_active
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Search(value) => {
                if self.dialog.is_none() {
                    self.query = value;
                }
            }
            Message::Open(index) => {
                self.open(index);
                if self.files_dock.is_some() {
                    return self.reload_files();
                }
            }
            Message::EditProfile(index) => {
                if let Some(profile) = self.profiles.get(index).cloned() {
                    self.editing_profile = Some(session_profile_key(&profile));
                    self.form = ConnectionForm::from_session(&profile);
                    self.dialog = Some(Dialog::Connection);
                    return operation::focus("connection-name");
                }
            }
            Message::SelectTab(index) => {
                self.activate_tab(index);
                if self.files_dock.is_some() {
                    return self.reload_files();
                }
            }
            Message::SelectNextTab => {
                if !self.tabs.is_empty() {
                    self.activate_tab((self.active + 1) % self.tabs.len());
                    if self.files_dock.is_some() {
                        return self.reload_files();
                    }
                }
            }
            Message::SelectPreviousTab => {
                if !self.tabs.is_empty() {
                    self.activate_tab((self.active + self.tabs.len() - 1) % self.tabs.len());
                    if self.files_dock.is_some() {
                        return self.reload_files();
                    }
                }
            }
            Message::New => {
                self.editing_profile = None;
                self.form = ConnectionForm::default();
                self.dialog = Some(Dialog::Connection);
                return operation::focus("connection-host");
            }
            Message::ImportProfiles => {
                self.import_path.clear();
                self.dialog = Some(Dialog::ImportProfiles);
                return operation::focus("import-path");
            }
            Message::ImportPath(value) => self.import_path = value,
            Message::ConfirmImportProfiles => {
                let path = self.import_path.trim();
                if path.is_empty() {
                    self.status = "Choose a profile import file path.".into();
                    return Task::none();
                }
                match import_sessions(
                    std::path::Path::new(path),
                    &self.profiles,
                    SessionImportMode::Merge,
                ) {
                    Ok(imported) => match save_sessions(&self.profiles_path, &imported) {
                        Ok(()) => {
                            let added = imported.len().saturating_sub(self.profiles.len());
                            self.profiles = imported;
                            self.dialog = None;
                            self.import_path.clear();
                            self.status =
                                format!("Imported {added} validated non-secret SSH profile(s).");
                        }
                        Err(error) => {
                            self.status = format!("Cannot save imported profiles: {error:#}");
                        }
                    },
                    Err(error) => {
                        self.status = format!("Profile import rejected: {error:#}");
                    }
                }
            }
            Message::Commands => {
                self.command_query.clear();
                self.dialog = Some(Dialog::Commands);
                return operation::focus("command-query");
            }
            Message::CommandQuery(value) => self.command_query = value,
            Message::CommandSender(value) => self.command_sender = value,
            Message::CommandStage(index) => {
                let items = iced_palette_items(&self.command_query, &self.snippets);
                if let Some(item) = items.get(index).cloned() {
                    match item {
                        PaletteItem::Snippet { index, name } => {
                            if let Some(snippet) = self.snippets.snippets.get(index) {
                                self.command_sender = snippet.body.clone();
                                self.status =
                                    format!("Snippet '{name}' staged. Press Send explicitly.");
                            }
                        }
                        PaletteItem::LocalAction { id, .. } => match id {
                            "connect" => {
                                self.dialog = None;
                                return self.update(Message::New);
                            }
                            "close-active" if !self.tabs.is_empty() => {
                                self.dialog = None;
                                return self.update(Message::AskClose(self.active));
                            }
                            _ => {}
                        },
                    }
                }
            }
            Message::CommandSend => {
                let text = self.command_sender.clone();
                if text.is_empty() {
                    self.status = "Command sender text is empty.".into();
                    return Task::none();
                }
                let Some(id) = self.focused_terminal_id() else {
                    self.status = "Focus a connected terminal before sending command text.".into();
                    return Task::none();
                };
                match terminal_ux::classify_paste(self.paste_policy, &text) {
                    PasteDecision::Send => {
                        if self.send_to_terminal(id, text.into_bytes()) {
                            self.status = "Command text sent to the focused terminal.".into();
                        }
                    }
                    PasteDecision::Confirm => {
                        self.dialog = Some(Dialog::PasteConfirm { id, text });
                        self.status =
                            "Multiline command text is waiting for explicit confirmation.".into();
                    }
                    PasteDecision::Block => {
                        self.status = "Command text blocked by the paste safety policy.".into();
                    }
                }
            }
            Message::SnippetName(value) => self.snippet_name = value,
            Message::SnippetBody(value) => self.snippet_body = value,
            Message::SaveSnippet => {
                let candidate = Snippet {
                    name: self.snippet_name.trim().to_owned(),
                    body: self.snippet_body.clone(),
                };
                match candidate.validate() {
                    Err(error) => self.status = format!("Invalid snippet: {error:#}"),
                    Ok(())
                        if self
                            .snippets
                            .snippets
                            .iter()
                            .any(|snippet| snippet.name.eq_ignore_ascii_case(&candidate.name)) =>
                    {
                        self.status = "Snippet name already exists.".into();
                    }
                    Ok(()) => {
                        self.snippets.snippets.push(candidate);
                        match command_palette::save_library(&self.snippets_path, &self.snippets) {
                            Ok(()) => {
                                self.snippet_name.clear();
                                self.snippet_body.clear();
                                self.status = "Snippet saved.".into();
                            }
                            Err(error) => {
                                self.snippets.snippets.pop();
                                self.status = format!("Cannot save snippet: {error:#}");
                            }
                        }
                    }
                }
            }
            Message::DeleteSnippet(index) => {
                if index < self.snippets.snippets.len() {
                    let removed = self.snippets.snippets.remove(index);
                    match command_palette::save_library(&self.snippets_path, &self.snippets) {
                        Ok(()) => self.status = format!("Snippet '{}' deleted.", removed.name),
                        Err(error) => {
                            self.snippets.snippets.insert(index, removed);
                            self.status = format!("Cannot delete snippet: {error:#}");
                        }
                    }
                }
            }
            Message::About => self.dialog = Some(Dialog::About),
            Message::CloseDialog => {
                self.dialog = None;
                self.editing_profile = None;
                self.form = ConnectionForm::default();
                self.files.name_input.clear();
            }
            Message::AskClose(index) => {
                let closing_key = self
                    .tabs
                    .get(index)
                    .map(|tab| session_profile_key(&tab.profile));
                let dirty_editor = closing_key.as_deref().is_some_and(|key| {
                    self.remote_editor
                        .as_ref()
                        .filter(|editor| editor.session_key == key)
                        .and_then(|editor| editor.handle.0.lock().ok())
                        .is_some_and(|editor| editor.is_dirty())
                });
                if dirty_editor {
                    self.status =
                        "Save or explicitly discard the remote editor changes before closing this SSH session."
                            .into();
                } else {
                    self.dialog = Some(Dialog::Close(index));
                }
            }
            Message::ConfirmClose(index) => {
                if index < self.tabs.len() {
                    let closing_key = session_profile_key(&self.tabs[index].profile);
                    let dirty_editor = self
                        .remote_editor
                        .as_ref()
                        .filter(|editor| editor.session_key == closing_key)
                        .and_then(|editor| editor.handle.0.lock().ok())
                        .is_some_and(|editor| editor.is_dirty());
                    if dirty_editor {
                        self.dialog = None;
                        self.status =
                            "Session close cancelled because the remote editor has unsaved changes."
                                .into();
                        return Task::none();
                    }
                    if self
                        .remote_editor
                        .as_ref()
                        .is_some_and(|editor| editor.session_key == closing_key)
                    {
                        self.remote_editor = None;
                    }
                    self.tabs.remove(index);
                    if index < self.active {
                        self.active -= 1;
                    }
                    self.active = self.active.min(self.tabs.len().saturating_sub(1));
                    if !self.tabs.is_empty() {
                        self.refresh_workspace_displays(self.active);
                    }
                }
                if self.tabs.is_empty() && self.files_dock.is_some() {
                    self.toggle_files();
                }
                self.dialog = None;
                if self.files_dock.is_some() && !self.tabs.is_empty() {
                    return self.reload_files();
                }
            }
            Message::ToggleFiles => {
                self.dialog = None;
                let opening = self.files_dock.is_none() && !self.tabs.is_empty();
                self.toggle_files();
                if opening && self.files_dock.is_some() {
                    return self.reload_files();
                }
            }
            Message::FilesRefresh => return self.reload_files(),
            Message::FilesLocalUp => {
                if let Some(parent) = self.files.local_dir.parent().map(ToOwned::to_owned) {
                    self.files.local_dir = parent;
                    self.files.selected_local = None;
                    return self.reload_local_files();
                }
            }
            Message::FilesRemoteUp => {
                let parent = remote_parent(&self.files.remote_dir);
                if parent != self.files.remote_dir {
                    self.files.remote_dir = parent;
                    self.files.selected_remote = None;
                    return self.reload_remote_files();
                }
            }
            Message::FilesOpenLocal(path) => {
                self.files.local_dir = path;
                self.files.selected_local = None;
                return self.reload_local_files();
            }
            Message::FilesOpenRemote(path) => {
                self.files.remote_dir = path;
                self.files.selected_remote = None;
                return self.reload_remote_files();
            }
            Message::FilesSelectLocal(path) => {
                self.files.selected_local = Some(path);
            }
            Message::FilesSelectRemote(entry) => {
                self.files.selected_remote = Some(entry);
            }
            Message::FilesRequestUpload => return self.request_upload(),
            Message::FilesRequestDownload => return self.request_download(),
            Message::FilesRequestDeleteLocal => {
                let Some(path) = self.files.selected_local.clone() else {
                    self.status = "Select a local file to delete.".into();
                    return Task::none();
                };
                self.dialog = Some(Dialog::FileDeleteLocal(path));
            }
            Message::FilesRequestDeleteRemote => {
                let Some(entry) = self.files.selected_remote.clone() else {
                    self.status = "Select a remote file to delete.".into();
                    return Task::none();
                };
                let Some(session) = self.tabs.get(self.active).map(|tab| tab.profile.clone())
                else {
                    return Task::none();
                };
                let session_key = session_profile_key(&session);
                let path = sftp::join_remote(&self.files.remote_dir, &entry.name);
                self.dialog = Some(Dialog::FileDeleteRemote {
                    session,
                    session_key,
                    path,
                    directory: entry.is_dir,
                });
            }
            Message::FilesRequestMkdirLocal => {
                self.files.name_input.clear();
                self.dialog = Some(Dialog::FileName(FileNameAction::MkdirLocal(
                    self.files.local_dir.clone(),
                )));
                return operation::focus("file-name");
            }
            Message::FilesRequestMkdirRemote => {
                let Some(session) = self.tabs.get(self.active).map(|tab| tab.profile.clone())
                else {
                    return Task::none();
                };
                self.files.name_input.clear();
                self.dialog = Some(Dialog::FileName(FileNameAction::MkdirRemote {
                    session_key: session_profile_key(&session),
                    session,
                    directory: self.files.remote_dir.clone(),
                }));
                return operation::focus("file-name");
            }
            Message::FilesRequestRenameLocal => {
                let Some(path) = self.files.selected_local.clone() else {
                    self.status = "Select a local file to rename.".into();
                    return Task::none();
                };
                self.files.name_input = path
                    .file_name()
                    .map(|value| value.to_string_lossy().into_owned())
                    .unwrap_or_default();
                self.dialog = Some(Dialog::FileName(FileNameAction::RenameLocal(path)));
                return operation::focus("file-name");
            }
            Message::FilesRequestRenameRemote => {
                let Some(entry) = self.files.selected_remote.clone() else {
                    self.status = "Select a remote file to rename.".into();
                    return Task::none();
                };
                let Some(session) = self.tabs.get(self.active).map(|tab| tab.profile.clone())
                else {
                    return Task::none();
                };
                self.files.name_input = entry.name.clone();
                self.dialog = Some(Dialog::FileName(FileNameAction::RenameRemote {
                    session_key: session_profile_key(&session),
                    session,
                    from: sftp::join_remote(&self.files.remote_dir, &entry.name),
                }));
                return operation::focus("file-name");
            }
            Message::FilesRequestEditRemote => {
                let Some(entry) = self.files.selected_remote.clone() else {
                    self.status = "Select a remote text file to edit.".into();
                    return Task::none();
                };
                if entry.is_dir {
                    self.status = "Select a remote file, not a directory.".into();
                    return Task::none();
                }
                let Some(session) = self.tabs.get(self.active).map(|tab| tab.profile.clone())
                else {
                    return Task::none();
                };
                let session_key = session_profile_key(&session);
                let remote = sftp::join_remote(&self.files.remote_dir, &entry.name);
                let expected_size = entry.size;
                let config = self.ssh_config.clone();
                let open_session = session.clone();
                let open_remote = remote.clone();
                self.status = format!("Opening remote editor for {remote}...");
                return Task::perform(
                    async move {
                        remote_edit::RemoteEdit::open(
                            &open_session,
                            config.as_deref(),
                            &open_remote,
                            expected_size,
                        )
                        .map(|editor| RemoteEditHandle(Arc::new(Mutex::new(editor))))
                        .map_err(|error| format!("{error:#}"))
                    },
                    move |result| Message::RemoteEditorOpened {
                        session,
                        session_key,
                        remote,
                        result,
                    },
                );
            }
            Message::RemoteEditorOpened {
                session,
                session_key,
                remote,
                result,
            } => match result {
                Ok(handle) => {
                    let active_session_key = self
                        .tabs
                        .get(self.active)
                        .map(|tab| session_profile_key(&tab.profile));
                    if active_session_key.as_deref() != Some(session_key.as_str()) {
                        self.status = format!(
                            "Finished opening {remote}, but its SSH session is no longer active; editor result was discarded."
                        );
                        return Task::none();
                    }
                    let text = handle
                        .0
                        .lock()
                        .map(|editor| editor.text().to_owned())
                        .unwrap_or_default();
                    self.remote_editor = Some(RemoteEditorState {
                        session,
                        session_key,
                        remote: remote.clone(),
                        handle,
                        content: text_editor::Content::with_text(&text),
                        saving: false,
                        conflict: false,
                        discard_confirm: false,
                        error: None,
                    });
                    self.status = format!("Opened {remote} in the private remote editor.");
                }
                Err(error) => {
                    self.status = format!("Remote editor could not open file: {error}");
                }
            },
            Message::RemoteEditorAction(action) => {
                if let Some(editor) = self.remote_editor.as_mut() {
                    editor.content.perform(action);
                    let text = editor.content.text();
                    if let Ok(mut remote) = editor.handle.0.lock() {
                        *remote.text_mut() = text;
                    }
                    editor.conflict = false;
                    editor.error = None;
                }
            }
            Message::RemoteEditorSave(force) => {
                let Some(editor) = self.remote_editor.as_mut() else {
                    return Task::none();
                };
                if editor.saving {
                    return Task::none();
                }
                editor.saving = true;
                editor.error = None;
                let handle = editor.handle.clone();
                let session = editor.session.clone();
                let config = self.ssh_config.clone();
                self.status = format!("Saving {}...", editor.remote);
                return Task::perform(
                    async move {
                        let mut remote = handle
                            .0
                            .lock()
                            .map_err(|_| "remote editor state lock is poisoned".to_string())?;
                        remote
                            .save(&session, config.as_deref(), force)
                            .map_err(|error| format!("{error:#}"))
                    },
                    Message::RemoteEditorSaved,
                );
            }
            Message::RemoteEditorSaved(result) => {
                let Some(editor) = self.remote_editor.as_mut() else {
                    return Task::none();
                };
                editor.saving = false;
                match result {
                    Ok(SaveOutcome::Saved) => {
                        editor.conflict = false;
                        editor.error = None;
                        self.status =
                            format!("Saved {} through staged SFTP replacement.", editor.remote);
                        if self.files_dock.is_some()
                            && self.files.session_key.as_deref()
                                == Some(editor.session_key.as_str())
                        {
                            return self.reload_remote_files();
                        }
                    }
                    Ok(SaveOutcome::Conflict) => {
                        editor.conflict = true;
                        self.status =
                            "Remote file changed; explicit overwrite confirmation is required."
                                .into();
                    }
                    Err(error) => {
                        editor.error = Some(error.clone());
                        self.status = format!("Remote editor save failed: {error}");
                    }
                }
            }
            Message::RemoteEditorClose => {
                let dirty = self
                    .remote_editor
                    .as_ref()
                    .and_then(|editor| editor.handle.0.lock().ok())
                    .is_some_and(|editor| editor.is_dirty());
                if dirty {
                    if let Some(editor) = self.remote_editor.as_mut() {
                        editor.discard_confirm = true;
                    }
                } else {
                    self.remote_editor = None;
                    self.status = "Remote editor closed.".into();
                }
            }
            Message::RemoteEditorDiscard => {
                let remote = self
                    .remote_editor
                    .as_ref()
                    .map(|editor| editor.remote.clone())
                    .unwrap_or_default();
                self.remote_editor = None;
                self.status = format!("Closed editor for {remote} without uploading changes.");
            }
            Message::RemoteEditorKeepEditing => {
                if let Some(editor) = self.remote_editor.as_mut() {
                    editor.discard_confirm = false;
                }
            }
            Message::RemoteEditorKeepConflict => {
                if let Some(editor) = self.remote_editor.as_mut() {
                    editor.conflict = false;
                }
            }
            Message::FilesNameChanged(value) => self.files.name_input = value,
            Message::ConfirmFileNameAction => {
                let name = self.files.name_input.trim().to_owned();
                if name.is_empty() {
                    self.status = "A file or folder name is required.".into();
                    return Task::none();
                }
                let Some(Dialog::FileName(action)) = self.dialog.take() else {
                    return Task::none();
                };
                self.files.name_input.clear();
                match action {
                    FileNameAction::MkdirLocal(directory) => {
                        return Task::perform(
                            async move {
                                sftp::mkdir_local(&directory, &name)
                                    .map(|path| format!("Created local {}", path.to_string_lossy()))
                                    .map_err(|error| format!("{error:#}"))
                            },
                            |result| Message::FilesMutationFinished {
                                remote: false,
                                session_key: None,
                                result,
                            },
                        );
                    }
                    FileNameAction::MkdirRemote {
                        session,
                        session_key,
                        directory,
                    } => {
                        let config = self.ssh_config.clone();
                        let path = sftp::join_remote(&directory, &name);
                        return Task::perform(
                            async move {
                                sftp::mkdir_remote(&session, config.as_deref(), &path)
                                    .map(|_| format!("Created remote {path}"))
                                    .map_err(|error| format!("{error:#}"))
                            },
                            move |result| Message::FilesMutationFinished {
                                remote: true,
                                session_key: Some(session_key.clone()),
                                result,
                            },
                        );
                    }
                    FileNameAction::RenameLocal(path) => {
                        return Task::perform(
                            async move {
                                sftp::rename_local(&path, &name)
                                    .map(|target| {
                                        format!("Renamed local {}", target.to_string_lossy())
                                    })
                                    .map_err(|error| format!("{error:#}"))
                            },
                            |result| Message::FilesMutationFinished {
                                remote: false,
                                session_key: None,
                                result,
                            },
                        );
                    }
                    FileNameAction::RenameRemote {
                        session,
                        session_key,
                        from,
                    } => {
                        let config = self.ssh_config.clone();
                        let to = sftp::join_remote(&remote_parent(&from), &name);
                        return Task::perform(
                            async move {
                                sftp::rename_remote(&session, config.as_deref(), &from, &to)
                                    .map(|_| format!("Renamed remote to {to}"))
                                    .map_err(|error| format!("{error:#}"))
                            },
                            move |result| Message::FilesMutationFinished {
                                remote: true,
                                session_key: Some(session_key.clone()),
                                result,
                            },
                        );
                    }
                }
            }
            Message::FilesMutationFinished {
                remote,
                session_key,
                result,
            } => {
                match result {
                    Ok(message) => {
                        self.status = message;
                        if remote {
                            self.files.selected_remote = None;
                        } else {
                            self.files.selected_local = None;
                        }
                    }
                    Err(error) => self.status = format!("File operation failed: {error}"),
                }
                let active_session = self.files.session_key.as_deref();
                if self.files_dock.is_some()
                    && (!remote || session_key.as_deref() == active_session)
                {
                    return if remote {
                        self.reload_remote_files()
                    } else {
                        self.reload_local_files()
                    };
                }
            }
            Message::FilesTransferPrepared(result) => match result {
                Ok(prepared) if prepared.conflict => {
                    let active_session = self.files.session_key.as_deref();
                    if active_session == Some(prepared.pending.session_key.as_str()) {
                        self.dialog = Some(Dialog::FileOverwrite(prepared.pending));
                    } else {
                        let label = match prepared.pending.direction {
                            TransferDirection::Upload => prepared
                                .pending
                                .local
                                .file_name()
                                .map(|value| format!("Upload {}", value.to_string_lossy()))
                                .unwrap_or_else(|| {
                                    format!("Upload {}", prepared.pending.local.to_string_lossy())
                                }),
                            TransferDirection::Download => {
                                format!("Download {}", prepared.pending.remote)
                            }
                        };
                        self.status = format!(
                            "{label} needs overwrite confirmation, but its SSH session is no longer active; switch back and retry the transfer."
                        );
                    }
                }
                Ok(prepared) => self.queue_transfer(prepared.pending, false),
                Err(error) => {
                    self.status = format!("Transfer could not be prepared: {error}");
                }
            },
            Message::ConfirmFileOverwrite => {
                if let Some(Dialog::FileOverwrite(pending)) = self.dialog.take() {
                    self.queue_transfer(pending, true);
                }
            }
            Message::ConfirmFileDelete => {
                let Some(dialog) = self.dialog.take() else {
                    return Task::none();
                };
                match dialog {
                    Dialog::FileDeleteLocal(path) => {
                        let label = path.to_string_lossy().into_owned();
                        return Task::perform(
                            async move {
                                sftp::delete_local(&path)
                                    .map(|_| format!("Deleted local {label}"))
                                    .map_err(|error| format!("{error:#}"))
                            },
                            |result| Message::FilesDeleteFinished {
                                remote: false,
                                session_key: None,
                                result,
                            },
                        );
                    }
                    Dialog::FileDeleteRemote {
                        session,
                        session_key,
                        path,
                        directory,
                    } => {
                        let config = self.ssh_config.clone();
                        let label = path.clone();
                        return Task::perform(
                            async move {
                                sftp::delete_remote(&session, config.as_deref(), &path, directory)
                                    .map(|_| format!("Deleted remote {label}"))
                                    .map_err(|error| format!("{error:#}"))
                            },
                            move |result| Message::FilesDeleteFinished {
                                remote: true,
                                session_key: Some(session_key.clone()),
                                result,
                            },
                        );
                    }
                    other => {
                        self.dialog = Some(other);
                    }
                }
            }
            Message::FilesDeleteFinished {
                remote,
                session_key,
                result,
            } => {
                match result {
                    Ok(message) => {
                        self.status = message;
                        if remote {
                            self.files.selected_remote = None;
                        } else {
                            self.files.selected_local = None;
                        }
                    }
                    Err(error) => {
                        self.status = format!("Delete failed: {error}");
                    }
                }
                let active_session = self.files.session_key.as_deref();
                if self.files_dock.is_some()
                    && (!remote || session_key.as_deref() == active_session)
                {
                    return if remote {
                        self.reload_remote_files()
                    } else {
                        self.reload_local_files()
                    };
                }
            }
            Message::CancelTransfer(id) => self.cancel_transfer(id),
            Message::RetryTransfer(id) => self.retry_transfer(id, false),
            Message::ResumeTransfer(id) => self.retry_transfer(id, true),
            Message::TransferBridgeReady(sender) => {
                self.transfer_bridge = Some(sender);
                self.try_start_next_transfer();
            }
            Message::TransferEvent(event) => {
                if self.handle_transfer_event(event) {
                    return self.reload_files();
                }
            }
            Message::FilesLocalLoaded(generation, result) => {
                if generation == self.files.local_generation {
                    self.files.local_loading = false;
                    match result {
                        Ok(entries) => {
                            self.files.local_entries = entries;
                            self.files.local_error = None;
                        }
                        Err(error) => {
                            self.files.local_entries.clear();
                            self.files.local_error = Some(error);
                        }
                    }
                }
            }
            Message::FilesRemoteLoaded(generation, result) => {
                if generation == self.files.remote_generation {
                    self.files.remote_loading = false;
                    match result {
                        Ok(entries) => {
                            self.files.remote_entries = entries;
                            self.files.remote_error = None;
                        }
                        Err(error) => {
                            self.files.remote_entries.clear();
                            self.files.remote_error = Some(error);
                        }
                    }
                }
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
            Message::TerminalSelectStart(pane_id, id, x, y) => {
                if let Some(tab) = self.tabs.get_mut(self.active) {
                    tab.focus = pane_id;
                }
                if self.command_terminal(
                    id,
                    egui_term::BackendCommand::SelectStart(egui_term::SelectionType::Simple, x, y),
                ) {
                    self.refresh_terminal_display(id);
                }
            }
            Message::TerminalSelectUpdate(id, x, y) => {
                if self.command_terminal(id, egui_term::BackendCommand::SelectUpdate(x, y)) {
                    self.refresh_terminal_display(id);
                }
            }
            Message::TerminalMouse(pane_id, id, button, modifiers, x, y, pressed) => {
                if let Some(tab) = self.tabs.get_mut(self.active) {
                    tab.focus = pane_id;
                }
                self.command_terminal(
                    id,
                    egui_term::BackendCommand::MouseReportAt(button, modifiers, x, y, pressed),
                );
            }
            Message::TerminalMouseWheel(pane_id, id, modifiers, x, y, lines) => {
                if let Some(tab) = self.tabs.get_mut(self.active) {
                    tab.focus = pane_id;
                }
                let button = if lines > 0 {
                    egui_term::MouseButton::ScrollUp
                } else {
                    egui_term::MouseButton::ScrollDown
                };
                for _ in 0..lines.unsigned_abs().min(8) {
                    self.command_terminal(
                        id,
                        egui_term::BackendCommand::MouseReportAt(button, modifiers, x, y, true),
                    );
                }
            }
            Message::TerminalScroll(id, lines) => {
                if self.command_terminal(id, egui_term::BackendCommand::Scroll(lines)) {
                    self.refresh_terminal_display(id);
                }
            }
            Message::CopySelection(id) => {
                if let Some(selection) = self.selected_terminal_text(id) {
                    self.status = "Terminal selection copied to clipboard.".into();
                    return iced::clipboard::write(selection);
                }
                self.status = "No terminal text is selected.".into();
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
            Message::Favorite(value) => self.form.favorite = value,
            Message::TagInput(value) => self.form.tag_input = value,
            Message::AddTag => {
                let tag = self.form.tag_input.trim();
                if tag.is_empty() {
                    self.form.error = Some("Tag cannot be empty.".into());
                } else if tag.len() > 64 || tag.chars().any(char::is_control) {
                    self.form.error =
                        Some("Tag must be at most 64 bytes without control characters.".into());
                } else if self.form.tags.len() >= 32 {
                    self.form.error = Some("At most 32 profile tags are supported.".into());
                } else if self
                    .form
                    .tags
                    .iter()
                    .any(|existing| existing.eq_ignore_ascii_case(tag))
                {
                    self.form.error = Some("That tag is already present.".into());
                } else {
                    self.form.tags.push(tag.to_owned());
                    self.form.tag_input.clear();
                    self.form.error = None;
                }
            }
            Message::RemoveTag(index) => {
                if index < self.form.tags.len() {
                    self.form.tags.remove(index);
                }
            }
            Message::StrictHostKey(value) => self.form.strict_host_key = value,
            Message::IdentityFile(value) => self.form.identity_file = value,
            Message::ProxyJump(value) => self.form.proxy_jump = value,
            Message::ProxyKind(value) => self.form.proxy_kind = value,
            Message::ProxyHost(value) => self.form.proxy_host = value,
            Message::ProxyPort(value) => self.form.proxy_port = value,
            Message::ProxyAuth(value) => self.form.proxy_auth = value,
            Message::Ciphers(value) => self.form.ciphers = value,
            Message::Macs(value) => self.form.macs = value,
            Message::KexAlgorithms(value) => self.form.kex_algorithms = value,
            Message::HostKeyAlgorithms(value) => self.form.host_key_algorithms = value,
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
                let wakeup = matches!(&event, egui_term::PtyEvent::Wakeup);
                let exited = matches!(
                    &event,
                    egui_term::PtyEvent::Exit | egui_term::PtyEvent::ChildExit(_)
                );
                match &event {
                    egui_term::PtyEvent::Title(title) => {
                        self.update_terminal_title(id, sanitize_terminal_title(title));
                    }
                    egui_term::PtyEvent::ResetTitle => {
                        self.update_terminal_title(id, None);
                    }
                    egui_term::PtyEvent::Bell => {
                        self.status = format!("Terminal {id} rang the bell.");
                    }
                    // Remote OSC clipboard requests stay isolated from the host clipboard.
                    egui_term::PtyEvent::ClipboardStore(_, _)
                    | egui_term::PtyEvent::ClipboardLoad(_, _) => {}
                    _ => {}
                }
                if wakeup || exited {
                    self.queue_terminal_refresh(id, exited);
                }
                if exited {
                    self.status = format!("Terminal {id} exited.");
                }
                if (wakeup || exited) && !self.terminal_frame_scheduled {
                    // One deferred frame for the entire window. Under concurrent SSH output,
                    // per-pane wakeup gates stay closed until the batch has been rendered.
                    self.terminal_frame_scheduled = true;
                    return Task::perform(
                        async {
                            thread::sleep(Duration::from_millis(12));
                        },
                        |_| Message::TerminalFrame,
                    );
                }
            }
            Message::TerminalFrame => {
                self.terminal_frame_scheduled = false;
                if !self.tabs.is_empty() {
                    self.refresh_workspace_displays(self.active);
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
                    keyboard::Key::Named(Named::Tab)
                        if modifiers.command() && modifiers.shift() =>
                    {
                        return self.update(Message::SelectPreviousTab);
                    }
                    keyboard::Key::Named(Named::Tab) if modifiers.command() => {
                        return self.update(Message::SelectNextTab);
                    }
                    keyboard::Key::Character("w") if modifiers.command() => {
                        if !self.tabs.is_empty() {
                            return self.update(Message::AskClose(self.active));
                        }
                    }
                    keyboard::Key::Character("n") if modifiers.command() => {
                        return self.update(Message::New);
                    }
                    keyboard::Key::Character("p") if modifiers.command() && modifiers.shift() => {
                        return self.update(Message::Commands);
                    }
                    keyboard::Key::Character("c")
                        if (cfg!(target_os = "macos") && modifiers.macos_command())
                            || (!cfg!(target_os = "macos")
                                && modifiers.control()
                                && modifiers.shift()) =>
                    {
                        if let Some(id) = self.focused_terminal_id() {
                            return self.update(Message::CopySelection(id));
                        }
                    }
                    keyboard::Key::Character("v")
                        if (cfg!(target_os = "macos") && modifiers.macos_command())
                            || (!cfg!(target_os = "macos")
                                && modifiers.control()
                                && modifiers.shift()) =>
                    {
                        return self.update(Message::RequestPaste);
                    }
                    _ => {}
                }

                let terminal_mode = self.focused_terminal_mode();
                if let Some(bytes) = terminal_key_bytes(
                    &key,
                    modifiers,
                    text.as_ref().map(|value| value.as_str()),
                    terminal_mode,
                ) {
                    self.send_to_focused_terminal(bytes);
                }
            }
            Message::Event(_) => {}
        }
        Task::none()
    }

    fn view(&self) -> Element<'_, Message> {
        let active_transfers = self
            .files
            .transfers
            .iter()
            .filter(|job| {
                matches!(
                    job.state,
                    TransferJobState::Queued | TransferJobState::Running
                )
            })
            .count();

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
                if active_transfers > 0 {
                    text(format!("{active_transfers} active transfer(s)"))
                        .size(12)
                        .color(BLUE)
                } else {
                    text("").size(12)
                },
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
            row![
                action("+  New connection", Message::New)
                    .style(primary)
                    .width(Fill),
                action("Import", Message::ImportProfiles),
            ]
            .spacing(6),
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
            let mut saved = column![].spacing(6).width(360);
            for (index, profile) in self
                .profiles
                .iter()
                .enumerate()
                .filter(|(_, profile)| profile.favorite)
                .chain(
                    self.profiles
                        .iter()
                        .enumerate()
                        .filter(|(_, profile)| !profile.favorite),
                )
                .take(4)
            {
                let destination = if profile.user.is_empty() {
                    profile.host.clone()
                } else {
                    format!("{}@{}", profile.user, profile.host)
                };
                saved = saved.push(
                    button(
                        row![
                            column![
                                text(profile.name.clone()).size(14),
                                text(destination).size(11).color(MUTED),
                            ]
                            .spacing(2),
                            space::horizontal(),
                            text(if profile.favorite { "PIN" } else { "SSH" })
                                .size(10)
                                .color(if profile.favorite { GREEN } else { BLUE }),
                        ]
                        .align_y(iced::Center),
                    )
                    .padding([9, 12])
                    .width(Fill)
                    .style(quiet)
                    .on_press(Message::Open(index)),
                );
            }

            let saved: Element<'_, Message> = if self.profiles.is_empty() {
                text("No saved SSH sessions yet.")
                    .size(12)
                    .color(MUTED)
                    .into()
            } else {
                column![
                    text("SAVED SESSIONS").size(11).color(MUTED),
                    saved,
                    text("Use the session sidebar for the complete library, folders and search.")
                        .size(11)
                        .color(MUTED),
                ]
                .spacing(8)
                .into()
            };

            return center(
                column![
                    text(">_").size(56).color(BLUE),
                    text("Your workspace, ready.").size(30),
                    text("Open a saved session or create a new connection.")
                        .size(16)
                        .color(MUTED),
                    space::vertical().height(8),
                    row![
                        action("+  New connection", Message::New).style(primary),
                        action("Import profiles", Message::ImportProfiles),
                    ]
                    .spacing(8),
                    space::vertical().height(10),
                    saved,
                    space::vertical().height(8),
                    text("The Iced shell uses the production OpenSSH PTY backend.")
                        .size(12)
                        .color(MUTED),
                ]
                .spacing(12)
                .align_x(iced::Center),
            )
            .into();
        }

        let mut tabs = row![].spacing(4);
        for (index, tab) in self.tabs.iter().enumerate() {
            let tab_label = tab
                .panes
                .get(tab.focus)
                .and_then(|pane| pane.terminal_title.as_deref())
                .unwrap_or(&tab.profile.name)
                .to_owned();
            tabs = tabs.push(
                container(
                    row![
                        action(tab_label, Message::SelectTab(index)).style(
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
                    text(
                        pane.terminal_title
                            .as_deref()
                            .unwrap_or(&pane.profile.name)
                            .to_owned()
                    )
                    .size(13),
                    text(state.0).size(10).color(state.1),
                    space::horizontal(),
                    action("Copy", Message::CopySelection(pane.id)),
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
            } else if let Some(snapshot) = &pane.display {
                let terminal_mode = pane
                    .terminal
                    .as_ref()
                    .map(|terminal| terminal.last_content().terminal_mode)
                    .unwrap_or_else(egui_term::TerminalMode::empty);
                canvas(TerminalCanvas {
                    pane: id,
                    id: pane.id,
                    generation: pane.display_generation,
                    terminal_mode,
                    snapshot,
                })
                .width(Fill)
                .height(Fill)
                .into()
            } else {
                let transcript = if pane.exited {
                    "Session exited without terminal output."
                } else {
                    "Connecting with system OpenSSH..."
                };
                text(transcript)
                    .font(Font::MONOSPACE)
                    .size(15)
                    .color(FG)
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

        let mut local_list = column![].spacing(3);
        if self.files.local_loading {
            local_list = local_list.push(text("Loading local directory...").size(12).color(MUTED));
        } else if let Some(error) = &self.files.local_error {
            local_list = local_list.push(text(error).size(12).color(DANGER));
        } else if self.files.local_entries.is_empty() {
            local_list = local_list.push(text("Directory is empty.").size(12).color(MUTED));
        } else {
            for entry in &self.files.local_entries {
                let kind = if entry.is_dir { "DIR " } else { "    " };
                let size = if entry.is_dir {
                    String::new()
                } else {
                    format_file_size(entry.size)
                };
                let label = format!("{kind}{:<38} {:>10}", display_leaf(&entry.name), size);
                if entry.is_dir {
                    local_list = local_list.push(
                        action(label, Message::FilesOpenLocal(entry.path.clone())).width(Fill),
                    );
                } else {
                    let selected = self.files.selected_local.as_ref() == Some(&entry.path);
                    let mut file =
                        action(label, Message::FilesSelectLocal(entry.path.clone())).width(Fill);
                    if selected {
                        file = file.style(selected_button);
                    }
                    local_list = local_list.push(file);
                }
            }
        }

        let mut remote_list = column![].spacing(3);
        if self.files.remote_loading {
            remote_list =
                remote_list.push(text("Loading remote directory...").size(12).color(MUTED));
        } else if let Some(error) = &self.files.remote_error {
            remote_list = remote_list.push(text(error).size(12).color(DANGER));
        } else if self.files.remote_entries.is_empty() {
            remote_list = remote_list.push(text("Directory is empty.").size(12).color(MUTED));
        } else {
            for entry in &self.files.remote_entries {
                let size = if entry.is_dir {
                    String::new()
                } else {
                    format_file_size(entry.size)
                };
                if entry.is_dir {
                    let path = sftp::join_remote(&self.files.remote_dir, &entry.name);
                    remote_list = remote_list.push(
                        action(
                            format!("DIR  {}", display_leaf(&entry.name)),
                            Message::FilesOpenRemote(path),
                        )
                        .width(Fill),
                    );
                } else {
                    let selected = self
                        .files
                        .selected_remote
                        .as_ref()
                        .is_some_and(|selected| selected.name == entry.name);
                    let mut file = action(
                        format!("{:<42} {:>10}", display_leaf(&entry.name), size),
                        Message::FilesSelectRemote(entry.clone()),
                    )
                    .width(Fill);
                    if selected {
                        file = file.style(selected_button);
                    }
                    remote_list = remote_list.push(file);
                }
            }
        }

        let local_panel = container(
            column![
                row![
                    text("LOCAL").size(11).color(MUTED),
                    text(self.files.local_dir.to_string_lossy().into_owned())
                        .size(12)
                        .color(FG),
                    space::horizontal(),
                    action("Up", Message::FilesLocalUp),
                ]
                .spacing(8)
                .align_y(iced::Center),
                scrollable(local_list).height(Fill),
            ]
            .spacing(8),
        )
        .padding(10)
        .width(Fill)
        .height(Fill)
        .style(card);

        let remote_panel = container(
            column![
                row![
                    text("REMOTE").size(11).color(BLUE),
                    text(&self.files.remote_dir).size(12).color(FG),
                    space::horizontal(),
                    action("Up", Message::FilesRemoteUp),
                ]
                .spacing(8)
                .align_y(iced::Center),
                scrollable(remote_list).height(Fill),
            ]
            .spacing(8),
        )
        .padding(10)
        .width(Fill)
        .height(Fill)
        .style(card);

        let activity = if self.files.local_loading || self.files.remote_loading {
            "Loading"
        } else if self.files.local_error.is_some() || self.files.remote_error.is_some() {
            "Needs attention"
        } else {
            "Ready"
        };

        let active_session = self.files.session_key.as_deref();
        let mut transfer_list = column![].spacing(5);
        let mut visible_transfers = 0usize;
        for job in self
            .files
            .transfers
            .iter()
            .rev()
            .filter(|job| active_session == Some(job.pending.session_key.as_str()))
            .take(6)
        {
            visible_transfers += 1;
            let state = match job.state {
                TransferJobState::Queued => "Queued",
                TransferJobState::Running => "Running",
                TransferJobState::Completed => "Completed",
                TransferJobState::Cancelled => "Cancelled",
                TransferJobState::Failed => "Failed",
            };
            let progress = match job.total {
                Some(total) if total > 0 => format!(
                    "{} / {} ({:.0}%)",
                    format_file_size(Some(job.transferred)),
                    format_file_size(Some(total)),
                    (job.transferred as f64 / total as f64 * 100.0).clamp(0.0, 100.0)
                ),
                _ if job.transferred > 0 => format_file_size(Some(job.transferred)),
                _ => String::new(),
            };

            let mut controls = row![].spacing(6).align_y(iced::Center);
            match job.state {
                TransferJobState::Queued | TransferJobState::Running => {
                    controls = controls.push(action("Cancel", Message::CancelTransfer(job.id)));
                }
                TransferJobState::Failed | TransferJobState::Cancelled => {
                    if job.resume_available {
                        controls = controls.push(action("Resume", Message::ResumeTransfer(job.id)));
                    }
                    controls = controls.push(action("Retry", Message::RetryTransfer(job.id)));
                }
                TransferJobState::Completed => {}
            }

            transfer_list = transfer_list.push(
                container(
                    row![
                        column![
                            text(job.label()).size(12),
                            text(if job.error.is_empty() {
                                format!("{state} {progress}").trim().to_owned()
                            } else {
                                format!("{state}: {}", job.error)
                            })
                            .size(11)
                            .color(
                                if job.state == TransferJobState::Failed {
                                    DANGER
                                } else {
                                    MUTED
                                }
                            ),
                        ]
                        .spacing(2),
                        space::horizontal(),
                        controls,
                    ]
                    .align_y(iced::Center),
                )
                .padding([6, 8])
                .width(Fill)
                .style(card),
            );
        }
        if visible_transfers == 0 {
            transfer_list =
                transfer_list.push(text("No transfers for this session.").size(11).color(MUTED));
        }

        container(
            column![
                row![
                    text("Files").size(16),
                    text(format!("Target: {target}")).size(12).color(BLUE),
                    text(activity).size(11).color(MUTED),
                    space::horizontal(),
                    action("Refresh", Message::FilesRefresh),
                ]
                .spacing(12)
                .align_y(iced::Center),
                row![local_panel, remote_panel].spacing(8).height(Fill),
                row![
                    action("Upload ->", Message::FilesRequestUpload),
                    action("<- Download", Message::FilesRequestDownload),
                    action("New local folder", Message::FilesRequestMkdirLocal),
                    action("New remote folder", Message::FilesRequestMkdirRemote),
                ]
                .spacing(8)
                .align_y(iced::Center),
                row![
                    action("Rename local", Message::FilesRequestRenameLocal),
                    action("Rename remote", Message::FilesRequestRenameRemote),
                    action("Edit remote", Message::FilesRequestEditRemote),
                    action("Delete local", Message::FilesRequestDeleteLocal),
                    action("Delete remote", Message::FilesRequestDeleteRemote),
                    space::horizontal(),
                    text("File actions reuse the validated SFTP/local-file policy; destructive actions still require confirmation.")
                        .size(11)
                        .color(MUTED),
                ]
                .spacing(8)
                .align_y(iced::Center),
                if let Some(editor) = self
                    .remote_editor
                    .as_ref()
                    .filter(|editor| active_session == Some(editor.session_key.as_str()))
                {
                    let dirty = editor
                        .handle
                        .0
                        .lock()
                        .ok()
                        .is_some_and(|remote| remote.is_dirty());
                    container(
                        column![
                            row![
                                text(format!("EDIT · {}", editor.remote)).size(12).color(BLUE),
                                if dirty {
                                    text("MODIFIED").size(10).color(DANGER)
                                } else {
                                    text("SAVED").size(10).color(GREEN)
                                },
                                space::horizontal(),
                                action(
                                    if editor.saving { "Saving..." } else { "Save" },
                                    Message::RemoteEditorSave(false)
                                ),
                                action("Close", Message::RemoteEditorClose),
                            ]
                            .spacing(8)
                            .align_y(iced::Center),
                            text_editor(&editor.content)
                                .on_action(Message::RemoteEditorAction)
                                .font(Font::MONOSPACE)
                                .height(220),
                            if editor.conflict {
                                container(
                                    row![
                                        text("Remote file changed since open.")
                                            .size(12)
                                            .color(DANGER),
                                        space::horizontal(),
                                        action(
                                            "Overwrite changed remote",
                                            Message::RemoteEditorSave(true)
                                        )
                                        .style(button::danger),
                                        action("Keep editing", Message::RemoteEditorKeepConflict),
                                    ]
                                    .spacing(8)
                                    .align_y(iced::Center)
                                )
                                .padding(8)
                                .style(card)
                            } else {
                                container(text("")).padding(0)
                            },
                            if editor.discard_confirm {
                                container(
                                    row![
                                        text("Discard unsaved editor changes?")
                                            .size(12)
                                            .color(DANGER),
                                        space::horizontal(),
                                        action("Discard", Message::RemoteEditorDiscard)
                                            .style(button::danger),
                                        action("Keep editing", Message::RemoteEditorKeepEditing),
                                    ]
                                    .spacing(8)
                                    .align_y(iced::Center)
                                )
                                .padding(8)
                                .style(card)
                            } else if let Some(error) = &editor.error {
                                container(text(error).size(11).color(DANGER))
                                    .padding(8)
                                    .style(card)
                            } else {
                                container(
                                    text("Saving re-checks the remote contents and uses staged replacement; closing never uploads automatically.")
                                        .size(11)
                                        .color(MUTED)
                                )
                                .padding(4)
                            },
                        ]
                        .spacing(6)
                    )
                    .padding(8)
                    .style(active_card)
                } else {
                    container(text(""))
                },
                container(
                    column![
                        text("TRANSFER QUEUE").size(11).color(MUTED),
                        transfer_list,
                    ]
                    .spacing(6)
                )
                .padding(8)
                .style(surface),
            ]
            .spacing(8)
            .height(Fill),
        )
        .padding(10)
        .height(Fill)
        .width(Fill)
        .style(card)
        .into()
    }

    fn dialog_view<'a>(&'a self, dialog: &'a Dialog) -> Element<'a, Message> {
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
                    row![
                        text("Favourite").size(13).color(MUTED),
                        action(
                            if self.form.favorite { "Pinned" } else { "Not pinned" },
                            Message::Favorite(!self.form.favorite)
                        )
                        .style(if self.form.favorite {
                            selected_button
                        } else {
                            quiet
                        }),
                    ]
                    .spacing(10)
                    .align_y(iced::Center),
                    column![
                        text("Tags").size(13).color(MUTED),
                        row![
                            text_input("Add profile tag", &self.form.tag_input)
                                .id("connection-tag")
                                .on_input(Message::TagInput)
                                .on_submit(Message::AddTag)
                                .padding(11),
                            action("Add", Message::AddTag),
                        ]
                        .spacing(8),
                        row(
                            self.form
                                .tags
                                .iter()
                                .enumerate()
                                .map(|(index, tag)| {
                                    action(
                                        format!("{tag}  x"),
                                        Message::RemoveTag(index)
                                    )
                                    .style(quiet)
                                    .into()
                                })
                                .collect::<Vec<Element<'_, Message>>>()
                        )
                        .spacing(6),
                    ]
                    .spacing(7),
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
                                text("Host key verification").size(11).color(MUTED),
                                row![
                                    action(
                                        "Ask on new/changed host",
                                        Message::StrictHostKey(false)
                                    )
                                    .style(if !self.form.strict_host_key {
                                        selected_button
                                    } else {
                                        quiet
                                    }),
                                    action(
                                        "Known hosts only",
                                        Message::StrictHostKey(true)
                                    )
                                    .style(if self.form.strict_host_key {
                                        selected_button
                                    } else {
                                        quiet
                                    }),
                                ]
                                .spacing(8),
                                text("Host-key verification is always enabled. “Known hosts only” refuses unknown hosts; “Ask” keeps OpenSSH's interactive trust prompt.")
                                    .size(11)
                                    .color(MUTED),
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
                                text("Structured proxy · mutually exclusive with ProxyJump")
                                    .size(11)
                                    .color(MUTED),
                                row![
                                    action(
                                        "None",
                                        Message::ProxyKind(ProxyKind::None)
                                    )
                                    .style(if self.form.proxy_kind == ProxyKind::None {
                                        selected_button
                                    } else {
                                        quiet
                                    }),
                                    action(
                                        "HTTP CONNECT",
                                        Message::ProxyKind(ProxyKind::HttpConnect)
                                    )
                                    .style(if self.form.proxy_kind == ProxyKind::HttpConnect {
                                        selected_button
                                    } else {
                                        quiet
                                    }),
                                    action(
                                        "SOCKS5",
                                        Message::ProxyKind(ProxyKind::Socks5)
                                    )
                                    .style(if self.form.proxy_kind == ProxyKind::Socks5 {
                                        selected_button
                                    } else {
                                        quiet
                                    }),
                                ]
                                .spacing(8),
                                if self.form.proxy_kind == ProxyKind::None {
                                    Element::<'_, Message>::from(column![])
                                } else {
                                    column![
                                        row![
                                            field(
                                                "Proxy host",
                                                "proxy.example or IP",
                                                &self.form.proxy_host,
                                                Message::ProxyHost
                                            ),
                                            container(field(
                                                "Proxy port",
                                                "8080",
                                                &self.form.proxy_port,
                                                Message::ProxyPort
                                            ))
                                            .width(140)
                                        ]
                                        .spacing(14),
                                        row![
                                            text("Proxy authentication")
                                                .size(12)
                                                .color(MUTED),
                                            action(
                                                "None",
                                                Message::ProxyAuth(ProxyAuth::None)
                                            )
                                            .style(if self.form.proxy_auth == ProxyAuth::None {
                                                selected_button
                                            } else {
                                                quiet
                                            }),
                                            action(
                                                "Environment credentials",
                                                Message::ProxyAuth(ProxyAuth::Environment)
                                            )
                                            .style(if self.form.proxy_auth == ProxyAuth::Environment {
                                                selected_button
                                            } else {
                                                quiet
                                            }),
                                        ]
                                        .spacing(8)
                                        .align_y(iced::Center),
                                        text("Environment mode reads ephemeral proxy credentials at runtime; usernames/passwords are never saved in the profile.")
                                            .size(11)
                                            .color(MUTED),
                                    ]
                                    .spacing(10)
                                    .into()
                                },
                                text("Algorithm policy · leave empty to inherit OpenSSH defaults")
                                    .size(11)
                                    .color(MUTED),
                                field(
                                    "Ciphers",
                                    "e.g. chacha20-poly1305@openssh.com,aes256-gcm@openssh.com",
                                    &self.form.ciphers,
                                    Message::Ciphers
                                ),
                                field(
                                    "MACs",
                                    "OpenSSH comma-separated policy",
                                    &self.form.macs,
                                    Message::Macs
                                ),
                                field(
                                    "Key exchange",
                                    "OpenSSH KexAlgorithms policy",
                                    &self.form.kex_algorithms,
                                    Message::KexAlgorithms
                                ),
                                field(
                                    "Host key algorithms",
                                    "OpenSSH HostKeyAlgorithms policy",
                                    &self.form.host_key_algorithms,
                                    Message::HostKeyAlgorithms
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
                                text("These values write directly into the existing Session::ssh model and use its current validation. Algorithm fields inherit OpenSSH/config defaults when empty; legacy algorithms still require explicit user input and retain the existing warning policy. Other advanced settings not exposed here yet are preserved when editing an existing profile.")
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
            Dialog::Commands => {
                let items = iced_palette_items(&self.command_query, &self.snippets);
                let mut palette = column![
                    text("Command palette").size(24),
                    text("Application actions are [APP]. Snippets are [REMOTE TEXT] and only stage text; they never execute automatically.")
                        .size(12)
                        .color(MUTED),
                    text_input("Search actions or snippets", &self.command_query)
                        .id("command-query")
                        .on_input(Message::CommandQuery)
                        .padding(11),
                ]
                .spacing(8);

                if items.is_empty() {
                    palette = palette.push(text("No matching commands.").size(12).color(MUTED));
                } else {
                    for (index, item) in items.iter().enumerate().take(12) {
                        let label = match item {
                            PaletteItem::LocalAction { label, .. } => format!("[APP] {label}"),
                            PaletteItem::Snippet { name, .. } => {
                                format!("[REMOTE TEXT] {name}")
                            }
                        };
                        palette =
                            palette.push(action(label, Message::CommandStage(index)).width(Fill));
                    }
                }

                let mut snippets = column![
                    text("REUSABLE SNIPPETS").size(11).color(MUTED),
                    text_input("Snippet name", &self.snippet_name)
                        .on_input(Message::SnippetName)
                        .padding(9),
                    text_input("Snippet body (non-secret remote text)", &self.snippet_body)
                        .on_input(Message::SnippetBody)
                        .padding(9),
                    action("Save snippet", Message::SaveSnippet),
                ]
                .spacing(6);
                for (index, snippet) in self.snippets.snippets.iter().enumerate().take(8) {
                    snippets = snippets.push(
                        row![
                            action(
                                format!("Stage · {}", snippet.name),
                                Message::CommandStage(
                                    iced_palette_items(&snippet.name, &self.snippets)
                                        .iter()
                                        .position(|item| matches!(
                                            item,
                                            PaletteItem::Snippet { index: item_index, .. }
                                                if *item_index == index
                                        ))
                                        .unwrap_or(usize::MAX)
                                )
                            ),
                            space::horizontal(),
                            action("Delete", Message::DeleteSnippet(index)),
                        ]
                        .spacing(6)
                        .align_y(iced::Center),
                    );
                }

                column![
                    palette,
                    container(
                        column![
                            text("COMMAND SENDER · FOCUSED PANE").size(11).color(BLUE),
                            text_input("Remote-shell text to stage", &self.command_sender)
                                .on_input(Message::CommandSender)
                                .on_submit(Message::CommandSend)
                                .padding(10),
                            row![
                                action("Send explicitly", Message::CommandSend).style(primary),
                                text("Multiline payloads still require the terminal paste confirmation.")
                                    .size(11)
                                    .color(MUTED),
                            ]
                            .spacing(10)
                            .align_y(iced::Center),
                        ]
                        .spacing(8)
                    )
                    .padding(10)
                    .style(card),
                    container(snippets).padding(10).style(card),
                    row![
                        action("New connection", Message::New),
                        action("Next tab", Message::SelectNextTab),
                        action("Split right", Message::Split(pane_grid::Axis::Vertical)),
                        action("Files", Message::ToggleFiles),
                        space::horizontal(),
                        action("Close", Message::CloseDialog),
                    ]
                    .spacing(7),
                ]
                .spacing(12)
                .into()
            }
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
            Dialog::FileOverwrite(pending) => {
                let direction = match pending.direction {
                    TransferDirection::Upload => "remote",
                    TransferDirection::Download => "local",
                };
                let source = match pending.direction {
                    TransferDirection::Upload => pending.local.to_string_lossy().into_owned(),
                    TransferDirection::Download => pending.remote.clone(),
                };
                let destination = match pending.direction {
                    TransferDirection::Upload => pending.remote.clone(),
                    TransferDirection::Download => pending.local.to_string_lossy().into_owned(),
                };
                column![
                    text("Replace existing file?").size(24),
                    text(format!(
                        "A file already exists at the {direction} destination."
                    ))
                    .color(MUTED),
                    container(
                        column![
                            text("Source").size(11).color(MUTED),
                            text(source).font(Font::MONOSPACE).size(12),
                            text("Destination").size(11).color(MUTED),
                            text(destination).font(Font::MONOSPACE).size(12),
                        ]
                        .spacing(6)
                    )
                    .padding(12)
                    .width(Fill)
                    .style(card),
                    text("Replacing is explicit. The transfer still uses the existing validated SFTP path and verification/resume policy.")
                        .size(12)
                        .color(MUTED),
                    row![
                        space::horizontal(),
                        action("Cancel", Message::CloseDialog),
                        action("Replace and transfer", Message::ConfirmFileOverwrite)
                            .style(button::danger)
                    ]
                    .spacing(10),
                ]
                .spacing(16)
                .into()
            }
            Dialog::FileDeleteLocal(path) => column![
                text("Delete local file?").size(24),
                text(path.to_string_lossy().into_owned())
                    .font(Font::MONOSPACE)
                    .size(12),
                text("This action is permanent. Directories are removed only when empty.")
                    .size(12)
                    .color(MUTED),
                row![
                    space::horizontal(),
                    action("Cancel", Message::CloseDialog),
                    action("Delete", Message::ConfirmFileDelete).style(button::danger)
                ]
                .spacing(10),
            ]
            .spacing(16)
            .into(),
            Dialog::FileDeleteRemote { path, directory, .. } => column![
                text(if *directory {
                    "Delete remote directory?"
                } else {
                    "Delete remote file?"
                })
                .size(24),
                text(path).font(Font::MONOSPACE).size(12),
                text(if *directory {
                    "This action is permanent. Remote directories are removed only when empty."
                } else {
                    "This action is permanent and uses the validated SFTP delete path."
                })
                .size(12)
                .color(MUTED),
                row![
                    space::horizontal(),
                    action("Cancel", Message::CloseDialog),
                    action("Delete", Message::ConfirmFileDelete).style(button::danger)
                ]
                .spacing(10),
            ]
            .spacing(16)
            .into(),
            Dialog::FileName(file_action) => {
                let (title, hint) = match file_action {
                    FileNameAction::MkdirLocal(_) => ("New local folder", "Folder name"),
                    FileNameAction::MkdirRemote { .. } => ("New remote folder", "Folder name"),
                    FileNameAction::RenameLocal(_) => ("Rename local file", "New filename"),
                    FileNameAction::RenameRemote { .. } => ("Rename remote file", "New filename"),
                };
                column![
                    text(title).size(24),
                    text_input(hint, &self.files.name_input)
                        .id("file-name")
                        .on_input(Message::FilesNameChanged)
                        .on_submit(Message::ConfirmFileNameAction)
                        .padding(11),
                    text("Names are validated by the existing SFTP/local-file policy before any operation is executed.")
                        .size(12)
                        .color(MUTED),
                    row![
                        space::horizontal(),
                        action("Cancel", Message::CloseDialog),
                        action("Apply", Message::ConfirmFileNameAction).style(primary)
                    ]
                    .spacing(10),
                ]
                .spacing(16)
                .into()
            }
            Dialog::ImportProfiles => column![
                text("Import SSH profiles").size(24),
                text("Merge a validated non-secret Inspirum profile JSON file into the current library. Existing profiles are never overwritten by import.")
                    .color(MUTED),
                column![
                    text("Import file").size(13).color(MUTED),
                    text_input("/path/to/profiles.json", &self.import_path)
                        .id("import-path")
                        .on_input(Message::ImportPath)
                        .on_submit(Message::ConfirmImportProfiles)
                        .padding(11)
                ]
                .spacing(7),
                text("Conflicting folder/name entries, invalid SSH policy, secret-bearing unsupported fields, oversized files, and malformed JSON are rejected before the active profile store is changed.")
                    .size(12)
                    .color(MUTED),
                row![
                    space::horizontal(),
                    action("Cancel", Message::CloseDialog),
                    action("Import and merge", Message::ConfirmImportProfiles).style(primary),
                ]
                .spacing(8),
            ]
            .spacing(14)
            .into(),
            Dialog::About => column![
                text("Production Iced migration").size(24),
                text("This is the production Iced migration shell. It reads and writes the validated Inspirum profile store, opens live SSH sessions through the existing OpenSSH/PTY backend, and uses the WindTerm-style workspace structure.\n\nThe terminal path includes guarded paste, reconnect, splits, selection/copy, scrollback and remote mouse support. The Files utility pane uses the existing validated SFTP backend for navigation, upload/download, overwrite confirmation, progress/cancel/retry/resume, rename, folder creation and confirmed delete.\n\nRemaining migration work is focused on deeper terminal fidelity, remote-editor parity, visual QA and native interaction regression testing.")
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
        "Proxy host" => "connection-proxy-host",
        "Proxy port" => "connection-proxy-port",
        "Ciphers" => "connection-ciphers",
        "MACs" => "connection-macs",
        "Key exchange" => "connection-kex",
        "Host key algorithms" => "connection-host-key-algorithms",
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
    fn terminal_mouse_motion_coalesces_within_a_cell() {
        let mut state = TerminalCanvasState::default();
        let cell_a = TerminalCanvas::report_cell(iced::Point::new(1.0, 1.0));
        let same_cell = TerminalCanvas::report_cell(iced::Point::new(
            TERMINAL_CELL_WIDTH - 0.01,
            TERMINAL_CELL_HEIGHT - 0.01,
        ));
        let next_column = TerminalCanvas::report_cell(iced::Point::new(
            TERMINAL_CELL_WIDTH,
            TERMINAL_CELL_HEIGHT - 0.01,
        ));
        let next_row = TerminalCanvas::report_cell(iced::Point::new(
            TERMINAL_CELL_WIDTH,
            TERMINAL_CELL_HEIGHT,
        ));
        assert_eq!(cell_a, same_cell);
        assert!(state.mark_cell_changed(cell_a));
        assert!(!state.mark_cell_changed(same_cell));
        assert!(state.mark_cell_changed(next_column));
        assert!(state.mark_cell_changed(next_row));
    }

    #[test]
    fn terminal_title_is_bounded_and_control_free() {
        assert_eq!(
            sanitize_terminal_title("  vim - server\u{7}  "),
            Some("vim - server".into())
        );
        let long = "x".repeat(500);
        assert_eq!(sanitize_terminal_title(&long).unwrap().chars().count(), 120);
        assert_eq!(sanitize_terminal_title("\n\r\t"), None);
    }

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
        session.strict = true;
        session.ssh.password_auth = Some(false);
        session.ssh.agent_forwarding = Some(true);
        session.ssh.identity_file = "/tmp/key".into();

        let mut form = ConnectionForm::from_session(&session);
        form.identity_file = "/tmp/new-key".into();
        let updated = form.session().expect("edited session");

        assert!(updated.strict);
        assert_eq!(updated.ssh.identity_file, "/tmp/new-key");
        assert_eq!(updated.ssh.password_auth, Some(false));
        assert_eq!(updated.ssh.agent_forwarding, Some(true));
    }

    #[test]
    fn connection_form_preserves_profile_favourite_and_tags() {
        let mut session = Session {
            name: "server".into(),
            host: "server.example".into(),
            favorite: true,
            tags: vec!["prod".into(), "radio".into()],
            ..Session::default()
        };

        let mut form = ConnectionForm::from_session(&session);
        assert!(form.favorite);
        assert_eq!(form.tags, session.tags);
        form.tags.push("night".into());

        let updated = form.session().expect("organized profile");
        assert!(updated.favorite);
        assert_eq!(updated.tags, vec!["prod", "radio", "night"]);

        session.favorite = false;
        let unpinned = ConnectionForm::from_session(&session)
            .session()
            .expect("unpinned profile");
        assert!(!unpinned.favorite);
    }

    #[test]
    fn connection_form_round_trips_advanced_algorithm_policy() {
        let mut session = Session {
            name: "server".into(),
            host: "server.example".into(),
            ..Session::default()
        };
        session.ssh.ciphers = "chacha20-poly1305@openssh.com".into();
        session.ssh.macs = "hmac-sha2-512-etm@openssh.com".into();
        session.ssh.kex_algorithms = "curve25519-sha256".into();
        session.ssh.host_key_algorithms = "ssh-ed25519".into();

        let form = ConnectionForm::from_session(&session);
        assert!(form.advanced);
        let updated = form.session().expect("advanced algorithm policy");

        assert_eq!(updated.ssh.ciphers, session.ssh.ciphers);
        assert_eq!(updated.ssh.macs, session.ssh.macs);
        assert_eq!(updated.ssh.kex_algorithms, session.ssh.kex_algorithms);
        assert_eq!(
            updated.ssh.host_key_algorithms,
            session.ssh.host_key_algorithms
        );

        let invalid = ConnectionForm {
            host: "server.example".into(),
            ciphers: "aes256-gcm@openssh.com, aes128-gcm@openssh.com".into(),
            ..Default::default()
        };
        assert!(invalid.session().is_err());
    }

    #[test]
    fn connection_form_round_trips_structured_proxy_without_credentials() {
        let mut session = Session {
            name: "server".into(),
            host: "server.example".into(),
            ..Session::default()
        };
        session.ssh.proxy_kind = ProxyKind::Socks5;
        session.ssh.proxy_host = "proxy.example".into();
        session.ssh.proxy_port = Some(1080);
        session.ssh.proxy_auth = ProxyAuth::Environment;

        let form = ConnectionForm::from_session(&session);
        assert!(form.advanced);
        let updated = form.session().expect("structured proxy");

        assert_eq!(updated.ssh.proxy_kind, ProxyKind::Socks5);
        assert_eq!(updated.ssh.proxy_host, "proxy.example");
        assert_eq!(updated.ssh.proxy_port, Some(1080));
        assert_eq!(updated.ssh.proxy_auth, ProxyAuth::Environment);

        let mut disabled = ConnectionForm::from_session(&session);
        disabled.proxy_kind = ProxyKind::None;
        let cleared = disabled.session().expect("disabled structured proxy");
        assert_eq!(cleared.ssh.proxy_kind, ProxyKind::None);
        assert!(cleared.ssh.proxy_host.is_empty());
        assert_eq!(cleared.ssh.proxy_port, None);
        assert_eq!(cleared.ssh.proxy_auth, ProxyAuth::None);
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
    fn terminal_keys_honor_application_cursor_mode_and_meta_input() {
        use keyboard::key::Named;

        let none = keyboard::Modifiers::default();
        assert_eq!(
            terminal_key_bytes(
                &keyboard::Key::Named(Named::ArrowUp),
                none,
                None,
                egui_term::TerminalMode::empty(),
            ),
            Some(b"\x1b[A".to_vec())
        );
        assert_eq!(
            terminal_key_bytes(
                &keyboard::Key::Named(Named::ArrowUp),
                none,
                None,
                egui_term::TerminalMode::APP_CURSOR,
            ),
            Some(b"\x1bOA".to_vec())
        );

        let shift = keyboard::Modifiers::SHIFT;
        assert_eq!(
            terminal_key_bytes(
                &keyboard::Key::Named(Named::Tab),
                shift,
                None,
                egui_term::TerminalMode::empty(),
            ),
            Some(b"\x1b[Z".to_vec())
        );

        let alt = keyboard::Modifiers::ALT;
        assert_eq!(
            terminal_key_bytes(
                &keyboard::Key::Character("x".into()),
                alt,
                Some("x"),
                egui_term::TerminalMode::empty(),
            ),
            Some(b"\x1bx".to_vec())
        );
    }

    #[test]
    fn terminal_keys_encode_xterm_cursor_modifiers_for_full_screen_apps() {
        use keyboard::key::Named;

        assert_eq!(
            terminal_key_bytes(
                &keyboard::Key::Named(Named::ArrowLeft),
                keyboard::Modifiers::CTRL,
                None,
                egui_term::TerminalMode::APP_CURSOR,
            ),
            Some(b"\x1b[1;5D".to_vec())
        );
        assert_eq!(
            terminal_key_bytes(
                &keyboard::Key::Named(Named::ArrowRight),
                keyboard::Modifiers::ALT | keyboard::Modifiers::SHIFT,
                None,
                egui_term::TerminalMode::empty(),
            ),
            Some(b"\x1b[1;4C".to_vec())
        );
        assert_eq!(
            terminal_key_bytes(
                &keyboard::Key::Named(Named::Home),
                keyboard::Modifiers::SHIFT,
                None,
                egui_term::TerminalMode::empty(),
            ),
            Some(b"\x1b[1;2H".to_vec())
        );
    }

    #[test]
    fn terminal_keys_send_committed_cjk_text_without_preedit_bytes() {
        let none = keyboard::Modifiers::default();
        let key = keyboard::Key::Character("中".into());

        assert_eq!(
            terminal_key_bytes(&key, none, Some("中文"), egui_term::TerminalMode::empty(),),
            Some("中文".as_bytes().to_vec())
        );

        assert_eq!(
            terminal_key_bytes(&key, none, None, egui_term::TerminalMode::empty(),),
            None
        );
    }

    #[test]
    fn pty_burst_schedules_only_one_deferred_frame_until_refresh() {
        let profiles_path = std::env::temp_dir().join(format!(
            "inspirum-iced-frame-test-{}-missing.json",
            std::process::id()
        ));
        let mut app = App::boot(profiles_path, None);
        assert!(!app.terminal_frame_scheduled);

        // Multiple sessions can wake in the same interval, but only one Iced
        // frame may be scheduled. No network connection is required.
        let _ = app.update(Message::PtyEvent(1001, egui_term::PtyEvent::Wakeup));
        assert!(app.terminal_frame_scheduled);
        let _ = app.update(Message::PtyEvent(1002, egui_term::PtyEvent::Wakeup));
        assert!(app.terminal_frame_scheduled);

        let _ = app.update(Message::TerminalFrame);
        assert!(!app.terminal_frame_scheduled);

        let _ = app.update(Message::PtyEvent(1001, egui_term::PtyEvent::Wakeup));
        assert!(app.terminal_frame_scheduled);
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
