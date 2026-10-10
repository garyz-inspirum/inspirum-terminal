//! Production Iced application shell.
//!
//! This module intentionally reuses the existing validated Session/profile model.
//! The frontend uses the live PTY backend and preserves
//! host-key, argv, paste, process-lifecycle, or transfer safeguards.
mod terminal_ime;
mod tools;

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
    collections::HashSet,
    path::PathBuf,
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
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
const FILES_PAGE_SIZE: usize = 200;
const PROFILE_PAGE_SIZE: usize = 100;

// Enable on demand with INSPIRUM_ICED_TRACE_MS=25. There is no per-frame
// console output and no clock read when the setting is absent.
static ICED_TRACE_THRESHOLD: OnceLock<Option<Duration>> = OnceLock::new();

fn parse_iced_trace_threshold(raw: &str) -> Option<Duration> {
    raw.trim()
        .parse::<u64>()
        .ok()
        .filter(|millis| *millis > 0)
        .map(Duration::from_millis)
}

fn iced_trace_threshold() -> Option<Duration> {
    *ICED_TRACE_THRESHOLD.get_or_init(|| {
        std::env::var("INSPIRUM_ICED_TRACE_MS")
            .ok()
            .and_then(|raw| parse_iced_trace_threshold(&raw))
    })
}

struct SlowIcedScope {
    stage: &'static str,
    started: Option<Instant>,
}

impl SlowIcedScope {
    fn start(stage: &'static str) -> Self {
        Self {
            stage,
            started: iced_trace_threshold().map(|_| Instant::now()),
        }
    }
}

impl Drop for SlowIcedScope {
    fn drop(&mut self) {
        if let (Some(started), Some(threshold)) = (self.started, iced_trace_threshold()) {
            let elapsed = started.elapsed();
            if elapsed >= threshold {
                // Intentionally never include Message data, hostnames, text,
                // clipboard contents, terminal output or filesystem paths.
                eprintln!(
                    "iced slow {}: {:.1} ms (threshold {} ms)",
                    self.stage,
                    elapsed.as_secs_f64() * 1000.0,
                    threshold.as_millis()
                );
            }
        }
    }
}

// This is a privacy curtain for the application window, not an OS screen lock.
fn privacy_lock_chord(key: &keyboard::Key, modifiers: keyboard::Modifiers) -> bool {
    modifiers.command()
        && modifiers.shift()
        && matches!(key.as_ref(), keyboard::Key::Character("l" | "L"))
}

fn focus_mode_chord(key: &keyboard::Key, modifiers: keyboard::Modifiers) -> bool {
    modifiers.alt()
        && !modifiers.control()
        && !modifiers.command()
        && matches!(
            key.as_ref(),
            keyboard::Key::Named(keyboard::key::Named::Enter)
        )
}

fn local_navigation_chord(key: &keyboard::Key, modifiers: keyboard::Modifiers) -> bool {
    modifiers.shift()
        && !modifiers.control()
        && !modifiers.alt()
        && !modifiers.command()
        && matches!(
            key.as_ref(),
            keyboard::Key::Named(keyboard::key::Named::Enter)
        )
}

// These optional vi-like shortcuts are active ONLY with local navigation
// explicitly enabled. Never interpret them while normal remote input is on.
fn local_navigation_scroll(key: &keyboard::Key, modifiers: keyboard::Modifiers) -> Option<i32> {
    use keyboard::key::Named;
    match key.as_ref() {
        keyboard::Key::Named(Named::ArrowUp) => Some(1),
        keyboard::Key::Named(Named::ArrowDown) => Some(-1),
        keyboard::Key::Named(Named::PageUp) => Some(16),
        keyboard::Key::Named(Named::PageDown) => Some(-16),
        keyboard::Key::Named(Named::Home) => Some(100_000),
        keyboard::Key::Named(Named::End) => Some(-100_000),
        keyboard::Key::Character("u")
            if modifiers.control() && !modifiers.alt() && !modifiers.shift() =>
        {
            Some(8)
        }
        keyboard::Key::Character("d")
            if modifiers.control() && !modifiers.alt() && !modifiers.shift() =>
        {
            Some(-8)
        }
        keyboard::Key::Character("k") if modifiers.is_empty() => Some(1),
        keyboard::Key::Character("j") if modifiers.is_empty() => Some(-1),
        keyboard::Key::Character("g") if modifiers.is_empty() => Some(100_000),
        keyboard::Key::Character("g" | "G") if modifiers.shift() => Some(-100_000),
        _ => None,
    }
}

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
            event::listen_with(|event, status, _| match &event {
                iced::Event::Keyboard(keyboard::Event::KeyPressed { key, modifiers, .. })
                    if privacy_lock_chord(key, *modifiers) =>
                {
                    Some(Message::TogglePrivacyLock)
                }
                iced::Event::Keyboard(keyboard::Event::KeyPressed { key, modifiers, .. })
                    if focus_mode_chord(key, *modifiers) =>
                {
                    Some(Message::ToggleFocusMode)
                }
                iced::Event::Keyboard(keyboard::Event::KeyPressed { key, modifiers, .. })
                    if status == event::Status::Ignored
                        && local_navigation_chord(key, *modifiers) =>
                {
                    Some(Message::ToggleLocalNavigation)
                }
                // A focused text field can consume Escape. Route only this
                // captured key to dialog dismissal; never leak it to SSH.
                iced::Event::Keyboard(keyboard::Event::KeyPressed {
                    key: keyboard::Key::Named(keyboard::key::Named::Escape),
                    ..
                }) if status == event::Status::Captured => Some(Message::DismissModalKey),
                iced::Event::Keyboard(keyboard::Event::KeyPressed { key, modifiers, .. })
                    if modifiers.command()
                        && matches!(
                            key.as_ref(),
                            keyboard::Key::Character(
                                "," | "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8"
                            )
                        ) =>
                {
                    Some(Message::Event(event))
                }
                iced::Event::Window(iced::window::Event::FileDropped(_)) => {
                    Some(Message::Event(event))
                }
                iced::Event::Keyboard(_) if status == event::Status::Ignored => {
                    Some(Message::Event(event))
                }
                _ => None,
            }),
            Subscription::run(pty_bridge),
            Subscription::run(transfer_bridge),
            Subscription::run(frame_bridge),
        ])
    })
    .run()
}

type FrameBridgeSender = std::sync::mpsc::Sender<()>;

/// Keep the 12 ms frame coalescing delay off Iced's async executor. One
/// persistent OS worker handles requests; no thread is spawned per redraw.
fn frame_timer_worker(
    requests: std::sync::mpsc::Receiver<()>,
    mut emit_frame: impl FnMut() -> bool,
) {
    while requests.recv().is_ok() {
        thread::sleep(Duration::from_millis(12));
        if !emit_frame() {
            break;
        }
    }
}

fn frame_bridge() -> impl Stream<Item = Message> {
    iced::stream::channel(8, async |mut output| {
        let (requests, receiver) = std::sync::mpsc::channel();
        let (tick_sender, mut ticks) = mpsc::unbounded();
        thread::spawn(move || {
            frame_timer_worker(receiver, || tick_sender.unbounded_send(()).is_ok());
        });
        if output
            .send(Message::FrameBridgeReady(requests))
            .await
            .is_err()
        {
            return;
        }
        while ticks.next().await.is_some() {
            if output.send(Message::TerminalFrame).await.is_err() {
                break;
            }
        }
    })
}

type PtyBridgeSender = mpsc::UnboundedSender<(u64, terminal_core::PtyEvent)>;

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

/// Run blocking SFTP and filesystem work on its own OS worker, rather than
/// occupying Iced's asynchronous executor. This keeps pointer, keyboard and
/// PTY event handling responsive while a remote host is slow or unreachable.
async fn run_blocking_result<T, F>(work: F) -> Result<T, String>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, String> + Send + 'static,
{
    let (sender, receiver) = iced::futures::channel::oneshot::channel();
    thread::spawn(move || {
        let _ = sender.send(work());
    });
    receiver
        .await
        .unwrap_or_else(|_| Err("Background file operation ended unexpectedly.".into()))
}

fn send_transfer_event(sender: &TransferBridgeSender, event: TransferEvent) {
    let _ = sender.unbounded_send(event);
}

struct TransferWorker {
    id: u64,
    pending: PendingTransfer,
    overwrite: bool,
    resume_path: Option<PathBuf>,
    resume_requested: bool,
    cancel: Arc<AtomicBool>,
    config: Option<PathBuf>,
    sender: TransferBridgeSender,
}

fn run_transfer_worker(worker: TransferWorker) {
    let TransferWorker {
        id,
        pending,
        overwrite,
        resume_path,
        resume_requested,
        cancel,
        config,
        sender,
    } = worker;
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
    sftp: bool,
    id: u64,
    profile: Session,
    terminal: Option<terminal_core::TerminalBackend>,
    display: Option<terminal_core::DisplaySnapshot>,
    display_generation: u64,
    display_dirty: bool,
    terminal_grid_size: Option<(u16, u16)>,
    cell_size: (f32, f32),
    canvas_size: Option<iced::Size>,
    terminal_title: Option<String>,
    error: Option<String>,
    exited: bool,
    refresh_pending: Arc<AtomicBool>,
}

struct Workspace {
    id: u64,
    profile: Session,
    panes: pane_grid::State<TerminalPane>,
    focus: pane_grid::Pane,
}

impl Workspace {
    fn new(profile: Session, terminal: TerminalPane) -> Self {
        let id = terminal.id;
        let (panes, focus) = pane_grid::State::new(terminal);
        Self {
            id,
            profile,
            panes,
            focus,
        }
    }

    fn split(&mut self, axis: pane_grid::Axis, terminal: TerminalPane) {
        if self.panes.len() < 4
            && let Some((pane, _)) = self.panes.split(axis, self.focus, terminal)
        {
            self.focus = pane;
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
    id: u64,
    session: Session,
    session_key: String,
    remote: String,
    handle: RemoteEditHandle,
    content: text_editor::Content,
    saving: bool,
    dirty: bool,
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
    local_visible: usize,
    remote_visible: usize,
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
            local_visible: FILES_PAGE_SIZE,
            remote_visible: FILES_PAGE_SIZE,
            selected_local: None,
            selected_remote: None,
            name_input: String::new(),
            transfers: Vec::new(),
        }
    }
}

fn next_file_page(current: usize, total: usize) -> usize {
    current.saturating_add(FILES_PAGE_SIZE).min(total)
}

fn next_profile_page(current: usize, total: usize) -> usize {
    current.saturating_add(PROFILE_PAGE_SIZE).min(total)
}

fn upload_conflicts_with_remote_entries(
    local: &std::path::Path,
    entries: &[sftp::RemoteEntry],
) -> bool {
    let Some(name) = local.file_name().and_then(|value| value.to_str()) else {
        return false;
    };
    // Scan once rather than cloning the entire remote listing into every
    // asynchronous upload request (which can contain tens of thousands of files).
    entries.iter().any(|entry| entry.name == name)
}

fn remote_parent(path: &str) -> String {
    let trimmed = path.trim_end_matches('/');
    if trimmed.is_empty() || trimmed == "." || trimmed == "/" {
        return if path.starts_with('/') {
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

fn normalized_paste_text(source: &str, normalize: bool) -> String {
    if normalize {
        source.replace("\r\n", "\n").replace('\r', "\n")
    } else {
        source.to_owned()
    }
}

fn paste_preview_text(text: &str, max_chars: usize) -> String {
    let mut preview = String::new();
    for ch in text.chars().take(max_chars) {
        match ch {
            '\n' | '\t' => preview.push(ch),
            ch if ch.is_control() => preview.push_str(&format!("\\u{{{:04x}}}", ch as u32)),
            ch => preview.push(ch),
        }
    }
    if text.chars().count() > max_chars {
        preview.push_str("\n... preview truncated ...");
    }
    preview
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
    Tools,
    Connection,
    Commands,
    Close(usize),
    PasteConfirm {
        id: u64,
        text: String,
        targets: Vec<u64>,
        normalize_line_endings: bool,
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
    Tool(tools::Action),
    Search(String),
    ShowMoreProfiles,
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
    CommandEdit(text_editor::Action),
    CommandStage(usize),
    CommandSend,
    SnippetName(String),
    SnippetEdit(text_editor::Action),
    EditSnippet(String),
    CancelSnippetEdit,
    SaveSnippet,
    DeleteSnippet(String),
    About,
    CloseDialog,
    DismissModalKey,
    AskClose(usize),
    ConfirmClose(usize),
    ToggleFiles,
    FilesRefresh,
    FilesLocalUp,
    FilesRemoteUp,
    FilesShowMoreLocal,
    FilesShowMoreRemote,
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
        request_id: u64,
        session: Session,
        session_key: String,
        remote: String,
        result: Result<RemoteEditHandle, String>,
    },
    RemoteEditorAction(text_editor::Action),
    RemoteEditorSave(bool),
    RemoteEditorSaved {
        editor_id: u64,
        result: Result<SaveOutcome, String>,
    },
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
    TogglePrivacyLock,
    ToggleFocusMode,
    ToggleLocalNavigation,
    RequestPaste,
    ClipboardRead(u64, Vec<u64>, Option<String>),
    ConfirmPaste,
    TogglePasteNormalization,
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
    PublicKeyAuth(Option<bool>),
    PasswordAuth(Option<bool>),
    KeyboardInteractiveAuth(Option<bool>),
    GssapiAuth(Option<bool>),
    AgentForwarding(Option<bool>),
    X11Forwarding(Option<bool>),
    Compression(Option<bool>),
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
    PtyEvent(u64, terminal_core::PtyEvent),
    TerminalImeCursor(u64, iced::Rectangle),
    TerminalImeCommit(u64, String),
    FrameBridgeReady(FrameBridgeSender),
    TerminalFrame,
    TransferBridgeReady(TransferBridgeSender),
    TransferEvent(TransferEvent),
    TerminalResized(u64, iced::Size),
    TerminalSelectStart(pane_grid::Pane, u64, f32, f32),
    TerminalSelectUpdate(u64, f32, f32),
    PanePaste(pane_grid::Pane, u64),
    SelectionFinished(u64, f32, f32, bool),
    TerminalMouse(
        pane_grid::Pane,
        u64,
        terminal_core::MouseButton,
        terminal_core::MouseModifiers,
        f32,
        f32,
        bool,
    ),
    TerminalMouseWheel(
        pane_grid::Pane,
        u64,
        terminal_core::MouseModifiers,
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
    pointer_hidden: bool,
    opacity: Cell<f32>,
    remote_button: Option<terminal_core::MouseButton>,
    modifiers: keyboard::Modifiers,
    last_position: Option<iced::Point>,
    last_report_cell: Option<(i32, i32)>,
    generation: Cell<u64>,
    row_hashes: RefCell<Vec<u64>>,
    row_caches: RefCell<Vec<canvas::Cache>>,
    background_cache: canvas::Cache,
    background_key: Cell<Option<(f32, f32, [u8; 3])>>,
    ime_cursor: Option<(u64, iced::Rectangle)>,
    ime_focused: bool,
}

impl Default for TerminalCanvasState {
    fn default() -> Self {
        Self {
            selecting: false,
            pointer_hidden: false,
            opacity: Cell::new(1.0),
            remote_button: None,
            modifiers: keyboard::Modifiers::default(),
            last_position: None,
            last_report_cell: None,
            generation: Cell::new(u64::MAX),
            row_hashes: RefCell::new(Vec::new()),
            row_caches: RefCell::new(Vec::new()),
            background_cache: canvas::Cache::new(),
            background_key: Cell::new(None),
            ime_cursor: None,
            ime_focused: false,
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

    /// Background geometry is independent of text/PTY generations. Refresh it
    /// only when the terminal background color or canvas bounds actually change.
    fn background_changed(&self, width: f32, height: f32, color: [u8; 3]) -> bool {
        let key = (width, height, color);
        if self.background_key.get() == Some(key) {
            return false;
        }
        self.background_key.set(Some(key));
        true
    }
}

struct TerminalCanvas<'a> {
    focused: bool,
    pane: pane_grid::Pane,
    id: u64,
    generation: u64,
    terminal_mode: terminal_core::TerminalMode,
    snapshot: &'a terminal_core::DisplaySnapshot,
    appearance: crate::appearance::TerminalAppearance,
}

impl TerminalCanvas<'_> {
    fn remote_mouse_enabled(&self, modifiers: keyboard::Modifiers) -> bool {
        self.terminal_mode
            .intersects(terminal_core::TerminalMode::MOUSE_MODE)
            && !modifiers.shift()
    }

    fn mouse_modifiers(modifiers: keyboard::Modifiers) -> terminal_core::MouseModifiers {
        terminal_core::MouseModifiers {
            shift: modifiers.shift(),
            alt: modifiers.alt(),
            command: modifiers.command(),
        }
    }

    fn mouse_button(button: mouse::Button) -> Option<terminal_core::MouseButton> {
        match button {
            mouse::Button::Left => Some(terminal_core::MouseButton::LeftButton),
            mouse::Button::Middle => Some(terminal_core::MouseButton::MiddleButton),
            mouse::Button::Right => Some(terminal_core::MouseButton::RightButton),
            _ => None,
        }
    }

    fn movement_button(button: terminal_core::MouseButton) -> terminal_core::MouseButton {
        match button {
            terminal_core::MouseButton::LeftButton => terminal_core::MouseButton::LeftMove,
            terminal_core::MouseButton::MiddleButton => terminal_core::MouseButton::MiddleMove,
            terminal_core::MouseButton::RightButton => terminal_core::MouseButton::RightMove,
            _ => terminal_core::MouseButton::NoneMove,
        }
    }

    #[cfg(test)]
    fn report_cell(position: iced::Point) -> (i32, i32) {
        (
            (position.x / TERMINAL_CELL_WIDTH).floor() as i32,
            (position.y / TERMINAL_CELL_HEIGHT).floor() as i32,
        )
    }
    fn cell_size(&self) -> (f32, f32) {
        terminal_cell_size(&self.appearance)
    }
    fn cell_at(&self, position: iced::Point) -> (i32, i32) {
        let (width, height) = self.cell_size();
        (
            (position.x / width).floor() as i32,
            (position.y / height).floor() as i32,
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

        if matches!(
            event,
            iced::Event::Window(iced::window::Event::RedrawRequested(_))
        ) {
            let was_focused = state.ime_focused;
            state.ime_focused = self.focused;
            if self.focused {
                let (width, height) = terminal_cell_size(&self.appearance);
                let cell = self.snapshot.cells.iter().find(|cell| cell.cursor);
                let rectangle = iced::Rectangle {
                    x: bounds.x + cell.map_or(0.0, |c| c.column as f32 * width),
                    y: bounds.y + cell.map_or(0.0, |c| c.row as f32 * height),
                    width,
                    height,
                };
                if !was_focused || state.ime_cursor != Some((self.id, rectangle)) {
                    state.ime_cursor = Some((self.id, rectangle));
                    return Some(canvas::Action::publish(Message::TerminalImeCursor(
                        self.id, rectangle,
                    )));
                }
            }
        }
        match event {
            iced::Event::Keyboard(keyboard::Event::KeyPressed { .. })
                if position.is_some() && self.appearance.hide_pointer_while_typing =>
            {
                state.pointer_hidden = true;
                Some(canvas::Action::request_redraw())
            }
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
                    state.last_report_cell = Some(self.cell_at(position));
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
                    state.last_report_cell = Some(self.cell_at(position));
                    Some(
                        canvas::Action::publish(Message::TerminalSelectStart(
                            self.pane, self.id, position.x, position.y,
                        ))
                        .and_capture(),
                    )
                } else if (*button == mouse::Button::Middle && self.appearance.middle_click_paste)
                    || (*button == mouse::Button::Right && self.appearance.right_click_paste)
                {
                    Some(
                        canvas::Action::publish(Message::PanePaste(self.pane, self.id))
                            .and_capture(),
                    )
                } else {
                    None
                }
            }
            iced::Event::Mouse(mouse::Event::CursorMoved { .. }) => {
                let was_hidden = state.pointer_hidden;
                state.pointer_hidden = false;
                let position = position?;
                if let Some(button) = state.remote_button {
                    if self.remote_mouse_enabled(state.modifiers)
                        && self.terminal_mode.intersects(
                            terminal_core::TerminalMode::MOUSE_DRAG
                                | terminal_core::TerminalMode::MOUSE_MOTION,
                        )
                    {
                        if !state.mark_cell_changed(self.cell_at(position)) {
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
                    if !state.mark_cell_changed(self.cell_at(position)) {
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
                        .contains(terminal_core::TerminalMode::MOUSE_MOTION)
                {
                    if !state.mark_cell_changed(self.cell_at(position)) {
                        return None;
                    }
                    Some(
                        canvas::Action::publish(Message::TerminalMouse(
                            self.pane,
                            self.id,
                            terminal_core::MouseButton::NoneMove,
                            Self::mouse_modifiers(state.modifiers),
                            position.x,
                            position.y,
                            true,
                        ))
                        .and_capture(),
                    )
                } else {
                    was_hidden.then(canvas::Action::request_redraw)
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
                        canvas::Action::publish(Message::SelectionFinished(
                            self.id,
                            position.x,
                            position.y,
                            self.appearance.select_to_copy,
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
                    mouse::ScrollDelta::Pixels { y, .. } => *y / self.cell_size().1,
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
            if state.pointer_hidden && self.appearance.hide_pointer_while_typing {
                mouse::Interaction::Hidden
            } else if state.selecting {
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
        let _slow = SlowIcedScope::start("terminal_canvas_draw");
        fn rgb(value: [u8; 3]) -> Color {
            Color::from_rgb8(value[0], value[1], value[2])
        }

        let (cell_width, cell_height) = self.cell_size();
        if state.generation.get() != self.generation {
            use std::{
                collections::hash_map::DefaultHasher,
                hash::{Hash, Hasher},
            };

            let appearance_key = format!("{:?}", self.appearance);
            let mut new_hashes = Vec::with_capacity(self.snapshot.rows);
            for row in 0..self.snapshot.rows {
                let mut hasher = DefaultHasher::new();
                appearance_key.hash(&mut hasher);
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
            state.generation.set(self.generation);
        }

        if state.opacity.replace(self.appearance.opacity) != self.appearance.opacity
            || state.background_changed(bounds.width, bounds.height, self.snapshot.background)
        {
            state.background_cache.clear();
        }

        let background_geometry = state
            .background_cache
            .draw(renderer, bounds.size(), |frame| {
                frame.fill(
                    &canvas::Path::rectangle(iced::Point::ORIGIN, bounds.size()),
                    Color {
                        a: self.appearance.opacity,
                        ..rgb(self.snapshot.background)
                    },
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
                    let x = cell.column as f32 * cell_width;
                    let y = cell.row as f32 * cell_height;
                    if x >= bounds.width || y >= bounds.height {
                        continue;
                    }

                    let cell_width = if cell.wide {
                        cell_width * 2.0
                    } else {
                        cell_width
                    };
                    let background = if cell.cursor
                        && self.appearance.cursor_style
                            == crate::appearance::TerminalCursorStyle::Block
                    {
                        cell.cursor_color
                    } else {
                        cell.background
                    };
                    if background != self.snapshot.background
                        || (cell.cursor
                            && self.appearance.cursor_style
                                == crate::appearance::TerminalCursorStyle::Block)
                    {
                        frame.fill(
                            &canvas::Path::rectangle(
                                iced::Point::new(x, y),
                                iced::Size::new(cell_width + 0.5, cell_height + 0.5),
                            ),
                            rgb(background),
                        );
                    }

                    if !matches!(cell.character, ' ' | '\t' | '\0') {
                        let mut terminal_font = match self.appearance.font_family {
                            crate::appearance::TerminalFontFamily::Monospace => Font::MONOSPACE,
                            crate::appearance::TerminalFontFamily::Proportional => Font::DEFAULT,
                        };
                        if cell.bold {
                            terminal_font.weight = font::Weight::Bold;
                        }
                        if cell.italic {
                            terminal_font.style = font::Style::Italic;
                        }
                        let foreground = if cell.cursor
                            && self.appearance.cursor_style
                                == crate::appearance::TerminalCursorStyle::Block
                        {
                            cell.background
                        } else {
                            cell.foreground
                        };
                        frame.fill_text(canvas::Text {
                            content: cell.character.to_string(),
                            position: iced::Point::new(x, y - 1.0),
                            color: rgb(foreground),
                            size: iced::Pixels(self.appearance.font_size),
                            font: terminal_font,
                            ..canvas::Text::default()
                        });
                    }

                    if cell.cursor
                        && self.appearance.cursor_style
                            != crate::appearance::TerminalCursorStyle::Block
                    {
                        let (offset, size) = match self.appearance.cursor_style {
                            crate::appearance::TerminalCursorStyle::Underline => (
                                iced::Point::new(x, y + cell_height - 2.0),
                                iced::Size::new(cell_width, 2.0),
                            ),
                            _ => (iced::Point::new(x, y), iced::Size::new(2.0, cell_height)),
                        };
                        frame.fill(
                            &canvas::Path::rectangle(offset, size),
                            rgb(cell.cursor_color),
                        );
                    }
                    if cell.underline {
                        frame.fill(
                            &canvas::Path::rectangle(
                                iced::Point::new(x, y + cell_height - 2.0),
                                iced::Size::new(cell_width, 1.0),
                            ),
                            rgb(cell.foreground),
                        );
                    }
                    if cell.strikeout {
                        frame.fill(
                            &canvas::Path::rectangle(
                                iced::Point::new(x, y + cell_height * 0.55),
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

fn terminal_cell_size(appearance: &crate::appearance::TerminalAppearance) -> (f32, f32) {
    (
        appearance.font_size * 0.6,
        appearance.font_size * (18.0 / 14.0),
    )
}

fn terminal_key_bytes(
    key: &keyboard::Key,
    modifiers: keyboard::Modifiers,
    committed_text: Option<&str>,
    terminal_mode: terminal_core::TerminalMode,
) -> Option<Vec<u8>> {
    use keyboard::key::Named;

    if modifiers.control()
        && let keyboard::Key::Character(value) = key.as_ref()
    {
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

    let app_cursor = terminal_mode.contains(terminal_core::TerminalMode::APP_CURSOR);
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
    public_key_auth: Option<bool>,
    password_auth: Option<bool>,
    keyboard_interactive_auth: Option<bool>,
    gssapi_auth: Option<bool>,
    agent_forwarding: Option<bool>,
    x11_forwarding: Option<bool>,
    compression: Option<bool>,
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
            public_key_auth: None,
            password_auth: None,
            keyboard_interactive_auth: None,
            gssapi_auth: None,
            agent_forwarding: None,
            x11_forwarding: None,
            compression: None,
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
            public_key_auth: session.ssh.public_key_auth,
            password_auth: session.ssh.password_auth,
            keyboard_interactive_auth: session.ssh.keyboard_interactive_auth,
            gssapi_auth: session.ssh.gssapi_auth,
            agent_forwarding: session.ssh.agent_forwarding,
            x11_forwarding: session.ssh.x11_forwarding,
            compression: session.ssh.compression,
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
                || session.ssh.public_key_auth.is_some()
                || session.ssh.password_auth.is_some()
                || session.ssh.keyboard_interactive_auth.is_some()
                || session.ssh.gssapi_auth.is_some()
                || session.ssh.agent_forwarding.is_some()
                || session.ssh.x11_forwarding.is_some()
                || session.ssh.compression.is_some()
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
        session.ssh.public_key_auth = self.public_key_auth;
        session.ssh.password_auth = self.password_auth;
        session.ssh.keyboard_interactive_auth = self.keyboard_interactive_auth;
        session.ssh.gssapi_auth = self.gssapi_auth;
        session.ssh.agent_forwarding = self.agent_forwarding;
        session.ssh.x11_forwarding = self.x11_forwarding;
        session.ssh.compression = self.compression;
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
    tools: tools::State,
    profiles_path: PathBuf,
    profiles_writable: bool,
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
    command_content: text_editor::Content,
    snippet_name: String,
    snippet_body: String,
    snippet_content: text_editor::Content,
    editing_snippet: Option<String>,
    sidebar_collapsed: bool,
    privacy_locked: bool,
    focus_mode: bool,
    local_navigation: HashSet<u64>,
    profile_visible: usize,
    profile_matches: Vec<usize>,
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
    ime_cursor: Option<(u64, iced::Rectangle)>,
    frame_bridge: Option<FrameBridgeSender>,
    transfer_bridge: Option<TransferBridgeSender>,
    next_terminal_id: u64,
    next_transfer_id: u64,
    next_remote_editor_id: u64,
    remote_editor_open_generation: u64,
}

impl App {
    fn boot(profiles_path: PathBuf, ssh_config: Option<PathBuf>) -> Self {
        let _slow = SlowIcedScope::start("startup");
        let (dock, terminal_dock) = pane_grid::State::new(Dock::Terminal);
        let (profiles, profiles_writable, mut load_error) = match load_sessions(&profiles_path) {
            Ok(profiles) => (profiles, true, None),
            Err(error) => (
                Vec::new(),
                false,
                Some(format!("Could not load profiles: {error:#}")),
            ),
        };
        // Save matching indices once so every terminal redraw does not rescan
        // and lowercase the full session library.
        let profile_matches = (0..profiles.len()).collect();
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
        let mut tools = tools::State::load(&profiles_path);
        for message in &tools.load_errors {
            tools.errors.record(message);
            load_error = Some(match load_error {
                Some(existing) => format!("{existing}\n{message}"),
                None => message.clone(),
            });
        }
        Self {
            tools,
            profiles_path,
            profiles_writable,
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
            command_content: text_editor::Content::new(),
            snippet_name: String::new(),
            snippet_body: String::new(),
            snippet_content: text_editor::Content::new(),
            editing_snippet: None,
            sidebar_collapsed: false,
            privacy_locked: false,
            focus_mode: false,
            local_navigation: HashSet::new(),
            profile_visible: PROFILE_PAGE_SIZE,
            profile_matches,
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
            ime_cursor: None,
            frame_bridge: None,
            transfer_bridge: None,
            next_terminal_id: 1,
            next_transfer_id: 1,
            next_remote_editor_id: 1,
            remote_editor_open_generation: 0,
        }
    }

    fn refresh_profile_matches(&mut self) {
        self.profile_matches = self
            .profiles
            .iter()
            .enumerate()
            .filter_map(|(index, profile)| {
                session_matches_query(profile, &self.query).then_some(index)
            })
            .collect();
    }

    fn new_terminal_pane(&mut self, profile: Session) -> TerminalPane {
        self.new_terminal_pane_kind(profile, false)
    }

    fn new_terminal_pane_kind(&mut self, profile: Session, sftp: bool) -> TerminalPane {
        let id = self.next_terminal_id;
        self.next_terminal_id = self.next_terminal_id.saturating_add(1);

        let mut pane = TerminalPane {
            sftp,
            id,
            profile,
            terminal: None,
            display: None,
            display_generation: 0,
            display_dirty: true,
            terminal_grid_size: None,
            cell_size: (TERMINAL_CELL_WIDTH, TERMINAL_CELL_HEIGHT),
            canvas_size: None,
            terminal_title: None,
            error: None,
            exited: false,
            refresh_pending: Arc::new(AtomicBool::new(false)),
        };

        // Iced subscriptions are initialized asynchronously. A fast click on a saved
        // profile must leave a pending connection, not a permanent error pane.
        if let Some(bridge) = &self.pty_bridge {
            Self::start_terminal_pane(&mut pane, bridge, self.ssh_config.as_deref());
        }
        pane
    }

    fn start_terminal_pane(
        pane: &mut TerminalPane,
        bridge: &PtyBridgeSender,
        config: Option<&std::path::Path>,
    ) {
        if pane.terminal.is_some() || pane.error.is_some() || pane.exited {
            return;
        }

        let bridge = Arc::new(Mutex::new(bridge.clone()));
        let refresh_pending = pane.refresh_pending.clone();
        let event_sink: Arc<dyn Fn(u64, terminal_core::PtyEvent) + Send + Sync> =
            Arc::new(move |id, event| {
                let is_wakeup = matches!(&event, terminal_core::PtyEvent::Wakeup);
                if is_wakeup && refresh_pending.swap(true, Ordering::AcqRel) {
                    return;
                }

                let sent = bridge
                    .lock()
                    .is_ok_and(|sender| sender.unbounded_send((id, event)).is_ok());
                if is_wakeup && !sent {
                    refresh_pending.store(false, Ordering::Release);
                }
            });

        let connection = if pane.sftp {
            terminal::connect_sftp_with_event_sink(pane.id, &pane.profile, config, event_sink)
        } else {
            terminal::connect_with_event_sink(pane.id, &pane.profile, config, event_sink)
        };
        match connection {
            Ok(mut terminal) => {
                // The UI may already have measured this pane while the bridge was
                // initializing. Apply that size to the newly started PTY.
                if let Some((columns, rows)) = pane.terminal_grid_size {
                    terminal.process_command(terminal_core::BackendCommand::Resize(
                        terminal_core::Size::new(
                            columns as f32 * pane.cell_size.0,
                            rows as f32 * pane.cell_size.1,
                        ),
                        terminal_core::Size::new(pane.cell_size.0, pane.cell_size.1),
                    ));
                }
                pane.terminal = Some(terminal);
            }
            Err(error) => {
                pane.error = Some(format!("{error:#}"));
            }
        }
    }

    fn send_to_focused_terminal(&mut self, bytes: Vec<u8>) {
        let Some(id) = self
            .tabs
            .get(self.active)
            .and_then(|t| t.panes.get(t.focus))
            .map(|p| p.id)
        else {
            return;
        };
        if self.local_navigation.contains(&id) {
            return;
        }
        let mut targets = self.tools.sync.destinations(id);
        targets.push(id);
        for target in targets {
            if self.active_terminal_matches(target) {
                self.command_terminal(target, terminal_core::BackendCommand::Write(bytes.clone()));
            }
        }
    }

    /// A delayed canvas event must never retarget a pane in another tab or
    /// a pane replaced by Reconnect. Pane handles alone are insufficient:
    /// every reconnect gets a new terminal ID.
    fn active_session_matches(&self, session_key: &str) -> bool {
        self.tabs
            .get(self.active)
            .is_some_and(|tab| session_profile_key(&tab.profile) == session_key)
    }

    fn active_editor_is_visible(&self) -> bool {
        self.remote_editor
            .as_ref()
            .is_some_and(|editor| self.active_session_matches(&editor.session_key))
    }

    fn active_pane_matches(&self, pane_id: pane_grid::Pane, id: u64) -> bool {
        self.tabs
            .get(self.active)
            .and_then(|tab| tab.panes.get(pane_id))
            .is_some_and(|pane| pane.id == id && !pane.exited)
    }

    fn active_terminal_matches(&self, id: u64) -> bool {
        self.tabs.get(self.active).is_some_and(|tab| {
            tab.panes
                .iter()
                .any(|(_, pane)| pane.id == id && !pane.exited)
        })
    }

    fn focused_terminal_id(&self) -> Option<u64> {
        let tab = self.tabs.get(self.active)?;
        let pane = tab.panes.get(tab.focus)?;
        (!pane.exited && pane.terminal.is_some()).then_some(pane.id)
    }

    fn focused_terminal_mode(&self) -> terminal_core::TerminalMode {
        self.tabs
            .get(self.active)
            .and_then(|tab| tab.panes.get(tab.focus))
            .and_then(|pane| pane.terminal.as_ref())
            .map(|terminal| terminal.last_content().terminal_mode)
            .unwrap_or_else(terminal_core::TerminalMode::empty)
    }

    fn focused_pane_matches(&self, id: u64) -> bool {
        self.tabs
            .get(self.active)
            .and_then(|tab| tab.panes.get(tab.focus))
            .is_some_and(|pane| pane.id == id && !pane.exited)
    }

    fn paste_targets(&self, id: u64) -> Vec<u64> {
        let mut targets = self.tools.sync.destinations(id);
        targets.retain(|target| self.active_terminal_matches(*target));
        targets.push(id);
        targets.sort_unstable();
        targets
    }
    fn send_to_terminal(&mut self, id: u64, bytes: Vec<u8>) -> bool {
        if !self.focused_pane_matches(id) || self.local_navigation.contains(&id) {
            return false;
        }
        let targets = self.paste_targets(id);
        let mut sent = false;
        for target in targets {
            let result =
                self.command_terminal(target, terminal_core::BackendCommand::Write(bytes.clone()));
            if target == id {
                sent = result;
            }
        }
        sent
    }

    fn command_terminal(&mut self, id: u64, command: terminal_core::BackendCommand) -> bool {
        // Enforce the mode at the single PTY write boundary, including
        // synchronized input, delayed clipboard callbacks and tool commands.
        if self.local_navigation.contains(&id)
            && matches!(
                &command,
                terminal_core::BackendCommand::Write(_)
                    | terminal_core::BackendCommand::MouseReportAt(..)
            )
        {
            return false;
        }
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

    fn contains_terminal_pane(&self, id: u64) -> bool {
        self.tabs
            .iter()
            .any(|tab| tab.panes.iter().any(|(_, pane)| pane.id == id))
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

    fn schedule_terminal_frame(&mut self) -> Task<Message> {
        if self.terminal_frame_scheduled {
            return Task::none();
        }
        // A dedicated timer worker enforces the 12 ms redraw gate without
        // occupying the async executor during typing, SFTP, or clipboard tasks.
        self.terminal_frame_scheduled = true;
        if self
            .frame_bridge
            .as_ref()
            .is_some_and(|bridge| bridge.send(()).is_ok())
        {
            return Task::none();
        }
        // During startup or after timer shutdown, refresh immediately rather
        // than leaving terminal output behind a permanently pending frame.
        Task::perform(async {}, |_| Message::TerminalFrame)
    }

    fn defer_terminal_refresh(&mut self, id: u64) -> Task<Message> {
        self.queue_terminal_refresh(id, false);
        self.schedule_terminal_frame()
    }

    fn refresh_workspace_displays(&mut self, index: usize) {
        let _slow = SlowIcedScope::start("terminal_snapshot");
        let Some(tab) = self.tabs.get_mut(index) else {
            return;
        };
        for (_, pane) in tab.panes.iter_mut() {
            if pane.display_dirty {
                if let Some(terminal) = pane.terminal.as_mut() {
                    let appearance = self
                        .tools
                        .appearance
                        .effective_for(&session_profile_key(&pane.profile));
                    let snapshot =
                        terminal.display_snapshot(&crate::appearance::terminal_theme(&appearance));
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
            if self.active != index {
                self.tools.sync.set_armed(false);
            }
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
        for tab in &mut self.tabs {
            for (_, pane) in tab.panes.iter_mut() {
                if pane.id != id {
                    continue;
                }
                let appearance = self
                    .tools
                    .appearance
                    .effective_for(&session_profile_key(&pane.profile));
                let cell = terminal_cell_size(&appearance);
                let columns = ((size.width - 28.0).max(1.0) / cell.0).floor().max(1.0) as u16;
                let lines = ((size.height - 28.0).max(1.0) / cell.1).floor().max(1.0) as u16;
                pane.canvas_size = Some(size);
                if pane.terminal_grid_size == Some((columns, lines)) && pane.cell_size == cell {
                    return;
                }
                pane.terminal_grid_size = Some((columns, lines));
                pane.cell_size = cell;
                if let Some(terminal) = pane.terminal.as_mut() {
                    terminal.process_command(terminal_core::BackendCommand::Resize(
                        terminal_core::Size::new(columns as f32 * cell.0, lines as f32 * cell.1),
                        terminal_core::Size::new(cell.0, cell.1),
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
            self.status = if self.pty_bridge.is_some() {
                "SSH session opened through the production OpenSSH/PTY backend.".into()
            } else {
                "SSH session queued until the terminal event bridge is ready.".into()
            };
        }
        self.dialog = None;
    }

    fn toggle_files(&mut self) {
        if let Some(pane) = self.files_dock.take() {
            self.dock.close(pane);
        } else if !self.tabs.is_empty()
            && let Some((pane, split)) =
                self.dock
                    .split(pane_grid::Axis::Horizontal, self.terminal_dock, Dock::Files)
        {
            self.files_dock = Some(pane);
            self.dock.resize(split, 0.62);
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
            self.files.remote_visible = FILES_PAGE_SIZE;
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
            run_blocking_result(move || {
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
            }),
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
            run_blocking_result(move || {
                sftp::list_remote(&profile, config.as_deref(), &directory)
                    .map_err(|error| format!("{error:#}"))
            }),
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
        let conflict = upload_conflicts_with_remote_entries(&local, &self.files.remote_entries);

        Task::perform(
            run_blocking_result(move || {
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
            }),
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
            run_blocking_result(move || {
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
            }),
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
                run_transfer_worker(TransferWorker {
                    id,
                    pending,
                    overwrite,
                    resume_path,
                    resume_requested,
                    cancel,
                    config,
                    sender,
                });
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

    fn stage_command(&mut self, text: String) {
        self.command_content = text_editor::Content::with_text(&text);
        self.command_sender = text;
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        let _slow = SlowIcedScope::start("event_update");
        if matches!(&message, Message::TogglePrivacyLock) {
            self.privacy_locked = !self.privacy_locked;
            self.ime_cursor = None;
            if self.privacy_locked {
                self.status = "Workspace hidden by privacy lock.".into();
            } else {
                if !self.tabs.is_empty() {
                    self.refresh_workspace_displays(self.active);
                }
                self.status = "Workspace visible again.".into();
            }
            return Task::none();
        }
        // The curtain does not suspend SSH or background transfers, but no
        // keyboard, IME, clipboard or command injection may reach a PTY while
        // the window is locked (including events queued before the lock).
        if self.privacy_locked
            && matches!(
                &message,
                Message::Event(_)
                    | Message::TerminalImeCommit(..)
                    | Message::RequestPaste
                    | Message::ClipboardRead(..)
                    | Message::ConfirmPaste
                    | Message::PanePaste(..)
                    | Message::TerminalMouse(..)
                    | Message::TerminalMouseWheel(..)
                    | Message::TerminalSelectStart(..)
                    | Message::TerminalSelectUpdate(..)
                    | Message::SelectionFinished(..)
                    | Message::TerminalScroll(..)
                    | Message::CopySelection(..)
                    | Message::CommandSend
                    | Message::ToggleFocusMode
                    | Message::ToggleLocalNavigation
                    | Message::Tool(
                        tools::Action::SendCommand
                            | tools::Action::Confirm
                            | tools::Action::TmuxAttach(..)
                            | tools::Action::TmuxCreate
                            | tools::Action::SftpTerminal
                            | tools::Action::SyncArm(..)
                    )
            )
        {
            return Task::none();
        }
        if matches!(&message, Message::ToggleLocalNavigation) {
            if self.dialog.is_some() || self.remote_editor.is_some() {
                return Task::none();
            }
            let id = self.tabs.get(self.active).and_then(|tab| {
                tab.panes
                    .get(tab.focus)
                    .filter(|pane| !pane.exited)
                    .map(|pane| pane.id)
            });
            if let Some(id) = id {
                if !self.local_navigation.insert(id) {
                    self.local_navigation.remove(&id);
                    self.status = "Remote input restored for focused pane.".into();
                } else {
                    self.ime_cursor = None;
                    self.status =
                        "Local navigation: arrows, PgUp/PgDn, j/k/g/G, Ctrl+u/d scroll. Shift+Enter exits."
                            .into();
                }
            } else {
                self.status = "Focus an SSH pane before changing navigation mode.".into();
            }
            return Task::none();
        }
        if matches!(&message, Message::ToggleFocusMode) {
            if self.tabs.is_empty() {
                self.status = "Open an SSH session before entering focus mode.".into();
            } else {
                // The existing tab, split panes and files dock remain intact.
                // Focus mode affects presentation only; it never retargets SSH.
                self.focus_mode = !self.focus_mode;
                self.status = if self.focus_mode {
                    "Focus mode: terminal panes only (Alt+Enter to exit).".into()
                } else {
                    "Full workspace restored.".into()
                };
            }
            return Task::none();
        }
        if !self.profiles_writable
            && matches!(&message, Message::Submit | Message::ConfirmImportProfiles)
        {
            let error =
                "Profile storage could not be loaded; repair it before saving or importing."
                    .to_owned();
            self.form.error = Some(error.clone());
            self.status = error;
            return Task::none();
        }
        match message {
            Message::TerminalImeCursor(id, cursor)
                if self.focused_pane_matches(id) && self.dialog.is_none() =>
            {
                self.ime_cursor = Some((id, cursor));
            }
            Message::TerminalImeCommit(id, text)
                if self.focused_pane_matches(id)
                    && self.dialog.is_none()
                    && self.remote_editor.is_none() =>
            {
                if let Some(bytes) = crate::keyboard::committed_text_bytes(&text) {
                    self.send_to_terminal(id, bytes);
                }
            }
            Message::TerminalImeCursor(..) | Message::TerminalImeCommit(..) => {}
            Message::TogglePrivacyLock => unreachable!("privacy lock handled before match"),
            Message::ToggleFocusMode => unreachable!("focus mode handled before match"),
            Message::ToggleLocalNavigation => {
                unreachable!("local navigation handled before match")
            }
            Message::Tool(action) => return self.tool_update(action),
            Message::Search(value) => {
                if self.dialog.is_none() || matches!(self.dialog, Some(Dialog::Tools)) {
                    self.query = value;
                    self.profile_visible = PROFILE_PAGE_SIZE;
                    self.refresh_profile_matches();
                }
            }
            Message::ShowMoreProfiles => {
                self.profile_visible =
                    next_profile_page(self.profile_visible, self.profile_matches.len());
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
                            self.refresh_profile_matches();
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
            Message::CommandEdit(action) => {
                self.command_content.perform(action);
                self.command_sender = self.command_content.text();
            }
            Message::CommandStage(index) => {
                let items = iced_palette_items(&self.command_query, &self.snippets);
                if let Some(item) = items.get(index).cloned() {
                    match item {
                        PaletteItem::Snippet { index, name } => {
                            if let Some(snippet) = self.snippets.snippets.get(index) {
                                self.stage_command(snippet.body.clone());
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
                            "quick-switch" => {
                                return self
                                    .tool_update(tools::Action::Open(tools::Panel::Workspace));
                            }
                            "search-history" => {
                                return self
                                    .tool_update(tools::Action::Open(tools::Panel::History));
                            }
                            "toggle-sync" => {
                                return self
                                    .tool_update(tools::Action::SyncArm(!self.tools.sync.armed()));
                            }
                            _ => {}
                        },
                    }
                }
            }
            Message::CommandSend => return self.tool_update(tools::Action::SendCommand),
            Message::SnippetName(value) => self.snippet_name = value,
            Message::SnippetEdit(action) => {
                self.snippet_content.perform(action);
                self.snippet_body = self.snippet_content.text();
            }
            Message::EditSnippet(name) => {
                if let Some(snippet) = self.snippets.snippets.iter().find(|s| s.name == name) {
                    self.snippet_name = snippet.name.clone();
                    self.snippet_body = snippet.body.clone();
                    self.snippet_content = text_editor::Content::with_text(&snippet.body);
                    self.editing_snippet = Some(name);
                }
            }
            Message::CancelSnippetEdit => {
                self.editing_snippet = None;
                self.snippet_name.clear();
                self.snippet_body.clear();
                self.snippet_content = text_editor::Content::new();
            }
            Message::SaveSnippet => {
                let candidate = Snippet {
                    name: self.snippet_name.trim().to_owned(),
                    body: self.snippet_body.clone(),
                };
                if let Err(error) = candidate.validate() {
                    self.status = format!("Invalid snippet: {error:#}");
                    return Task::none();
                }
                let editing = self
                    .editing_snippet
                    .as_ref()
                    .and_then(|name| self.snippets.snippets.iter().position(|s| s.name == *name));
                if self.editing_snippet.is_some() && editing.is_none() {
                    self.status = "The snippet being edited has changed or was removed. Reload it before saving.".into();
                    return Task::none();
                }
                if self.snippets.snippets.iter().enumerate().any(|(i, s)| {
                    Some(i) != editing && s.name.eq_ignore_ascii_case(&candidate.name)
                }) {
                    self.status = "Snippet name already exists.".into();
                    return Task::none();
                }
                let mut library = self.snippets.clone();
                if let Some(index) = editing {
                    library.snippets[index] = candidate;
                } else {
                    library.snippets.push(candidate);
                }
                match command_palette::save_library(&self.snippets_path, &library) {
                    Ok(()) => {
                        self.snippets = library;
                        let _ = self.update(Message::CancelSnippetEdit);
                        self.status = "Snippet saved.".into();
                    }
                    Err(error) => self.status = format!("Cannot save snippet: {error:#}"),
                }
            }
            Message::DeleteSnippet(name) => {
                if let Some(index) = self.snippets.snippets.iter().position(|s| s.name == name) {
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
            Message::DismissModalKey => {
                if self.dialog.is_some() {
                    return self.update(Message::CloseDialog);
                }
            }
            Message::AskClose(index) => {
                let closing_key = self
                    .tabs
                    .get(index)
                    .map(|tab| session_profile_key(&tab.profile));
                let dirty_editor = closing_key.as_deref().is_some_and(|key| {
                    self.remote_editor.as_ref().is_some_and(|editor| {
                        editor.session_key == key && (editor.dirty || editor.saving)
                    })
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
                self.tools.sync.set_armed(false);
                if index < self.tabs.len() {
                    let closing_key = session_profile_key(&self.tabs[index].profile);
                    let dirty_editor = self.remote_editor.as_ref().is_some_and(|editor| {
                        editor.session_key == closing_key && (editor.dirty || editor.saving)
                    });
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
                    self.prune_tool_panes();
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
                    self.files.local_visible = FILES_PAGE_SIZE;
                    self.files.selected_local = None;
                    return self.reload_local_files();
                }
            }
            Message::FilesRemoteUp => {
                let parent = remote_parent(&self.files.remote_dir);
                if parent != self.files.remote_dir {
                    self.files.remote_dir = parent;
                    self.files.remote_visible = FILES_PAGE_SIZE;
                    self.files.selected_remote = None;
                    return self.reload_remote_files();
                }
            }
            Message::FilesOpenLocal(path) => {
                self.files.local_dir = path;
                self.files.local_visible = FILES_PAGE_SIZE;
                self.files.selected_local = None;
                return self.reload_local_files();
            }
            Message::FilesOpenRemote(path) => {
                self.files.remote_dir = path;
                self.files.remote_visible = FILES_PAGE_SIZE;
                self.files.selected_remote = None;
                return self.reload_remote_files();
            }
            Message::FilesShowMoreLocal => {
                self.files.local_visible =
                    next_file_page(self.files.local_visible, self.files.local_entries.len());
            }
            Message::FilesShowMoreRemote => {
                self.files.remote_visible =
                    next_file_page(self.files.remote_visible, self.files.remote_entries.len());
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
                if self
                    .remote_editor
                    .as_ref()
                    .is_some_and(|editor| editor.saving || editor.dirty)
                {
                    self.status = "Save or explicitly discard the current remote editor before opening another file.".into();
                    return Task::none();
                }
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
                self.remote_editor_open_generation =
                    self.remote_editor_open_generation.wrapping_add(1);
                let request_id = self.remote_editor_open_generation;
                self.status = format!("Opening remote editor for {remote}...");
                return Task::perform(
                    run_blocking_result(move || {
                        remote_edit::RemoteEdit::open(
                            &open_session,
                            config.as_deref(),
                            &open_remote,
                            expected_size,
                        )
                        .map(|editor| RemoteEditHandle(Arc::new(Mutex::new(editor))))
                        .map_err(|error| format!("{error:#}"))
                    }),
                    move |result| Message::RemoteEditorOpened {
                        request_id,
                        session,
                        session_key,
                        remote,
                        result,
                    },
                );
            }
            Message::RemoteEditorOpened {
                request_id,
                session,
                session_key,
                remote,
                result,
            } => {
                if request_id != self.remote_editor_open_generation {
                    return Task::none();
                }
                match result {
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
                        if self
                            .remote_editor
                            .as_ref()
                            .is_some_and(|editor| editor.saving || editor.dirty)
                        {
                            self.status =
                                "Remote editor open result ignored to protect unsaved changes."
                                    .into();
                            return Task::none();
                        }
                        let text = handle
                            .0
                            .lock()
                            .map(|editor| editor.text().to_owned())
                            .unwrap_or_default();
                        let id = self.next_remote_editor_id;
                        self.next_remote_editor_id = self.next_remote_editor_id.saturating_add(1);
                        self.remote_editor = Some(RemoteEditorState {
                            id,
                            session,
                            session_key,
                            remote: remote.clone(),
                            handle,
                            content: text_editor::Content::with_text(&text),
                            saving: false,
                            dirty: false,
                            conflict: false,
                            discard_confirm: false,
                            error: None,
                        });
                        self.status = format!("Opened {remote} in the private remote editor.");
                    }
                    Err(error) => {
                        self.status = format!("Remote editor could not open file: {error}");
                    }
                }
            }
            Message::RemoteEditorAction(action) => {
                if !self.active_editor_is_visible() {
                    return Task::none();
                }
                if let Some(editor) = self.remote_editor.as_mut() {
                    // Saving holds the mutex through network I/O. Never wait for it
                    // in a UI input callback.
                    if editor.saving {
                        return Task::none();
                    }
                    editor.content.perform(action);
                    let text = editor.content.text();
                    if let Ok(mut remote) = editor.handle.0.lock() {
                        *remote.text_mut() = text;
                        editor.dirty = remote.is_dirty();
                    } else {
                        editor.dirty = true;
                    }
                    editor.conflict = false;
                    editor.error = None;
                }
            }
            Message::RemoteEditorSave(force) => {
                if !self.active_editor_is_visible() {
                    return Task::none();
                }
                let Some(editor) = self.remote_editor.as_mut() else {
                    return Task::none();
                };
                if editor.saving {
                    return Task::none();
                }
                editor.saving = true;
                editor.error = None;
                let handle = editor.handle.clone();
                let editor_id = editor.id;
                let session = editor.session.clone();
                let config = self.ssh_config.clone();
                self.status = format!("Saving {}...", editor.remote);
                return Task::perform(
                    run_blocking_result(move || {
                        let mut remote = handle
                            .0
                            .lock()
                            .map_err(|_| "remote editor state lock is poisoned".to_string())?;
                        remote
                            .save(&session, config.as_deref(), force)
                            .map_err(|error| format!("{error:#}"))
                    }),
                    move |result| Message::RemoteEditorSaved { editor_id, result },
                );
            }
            Message::RemoteEditorSaved { editor_id, result } => {
                let Some(editor) = self.remote_editor.as_mut() else {
                    return Task::none();
                };
                if editor.id != editor_id {
                    return Task::none();
                }
                editor.saving = false;
                match result {
                    Ok(SaveOutcome::Saved) => {
                        editor.dirty = false;
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
                if !self.active_editor_is_visible() {
                    return Task::none();
                }
                if self
                    .remote_editor
                    .as_ref()
                    .is_some_and(|editor| editor.saving)
                {
                    self.status =
                        "Remote editor is saving; close it once the save finishes.".into();
                    return Task::none();
                }
                let dirty = self
                    .remote_editor
                    .as_ref()
                    .is_some_and(|editor| editor.dirty);
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
                if !self.active_editor_is_visible() {
                    return Task::none();
                }
                if self
                    .remote_editor
                    .as_ref()
                    .is_some_and(|editor| editor.saving)
                {
                    self.status = "Remote editor is saving; changes cannot be discarded during an in-flight upload.".into();
                    return Task::none();
                }
                let remote = self
                    .remote_editor
                    .as_ref()
                    .map(|editor| editor.remote.clone())
                    .unwrap_or_default();
                self.remote_editor = None;
                self.status = format!("Closed editor for {remote} without uploading changes.");
            }
            Message::RemoteEditorKeepEditing => {
                if !self.active_editor_is_visible() {
                    return Task::none();
                }
                if let Some(editor) = self.remote_editor.as_mut() {
                    editor.discard_confirm = false;
                }
            }
            Message::RemoteEditorKeepConflict => {
                if !self.active_editor_is_visible() {
                    return Task::none();
                }
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
                            run_blocking_result(move || {
                                sftp::mkdir_local(&directory, &name)
                                    .map(|path| format!("Created local {}", path.to_string_lossy()))
                                    .map_err(|error| format!("{error:#}"))
                            }),
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
                            run_blocking_result(move || {
                                sftp::mkdir_remote(&session, config.as_deref(), &path)
                                    .map(|_| format!("Created remote {path}"))
                                    .map_err(|error| format!("{error:#}"))
                            }),
                            move |result| Message::FilesMutationFinished {
                                remote: true,
                                session_key: Some(session_key.clone()),
                                result,
                            },
                        );
                    }
                    FileNameAction::RenameLocal(path) => {
                        return Task::perform(
                            run_blocking_result(move || {
                                sftp::rename_local(&path, &name)
                                    .map(|target| {
                                        format!("Renamed local {}", target.to_string_lossy())
                                    })
                                    .map_err(|error| format!("{error:#}"))
                            }),
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
                            run_blocking_result(move || {
                                sftp::rename_remote(&session, config.as_deref(), &from, &to)
                                    .map(|_| format!("Renamed remote to {to}"))
                                    .map_err(|error| format!("{error:#}"))
                            }),
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
                            run_blocking_result(move || {
                                sftp::delete_local(&path)
                                    .map(|_| format!("Deleted local {label}"))
                                    .map_err(|error| format!("{error:#}"))
                            }),
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
                            run_blocking_result(move || {
                                sftp::delete_remote(&session, config.as_deref(), &path, directory)
                                    .map(|_| format!("Deleted remote {label}"))
                                    .map_err(|error| format!("{error:#}"))
                            }),
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
                if self
                    .tabs
                    .get(self.active)
                    .and_then(|tab| tab.panes.get(tab.focus))
                    .is_some_and(|pane| self.local_navigation.contains(&pane.id))
                {
                    self.status = "Switch to remote input before pasting.".into();
                    return Task::none();
                }
                let Some(id) = self.focused_terminal_id() else {
                    self.status = "Focus a connected terminal before pasting.".into();
                    return Task::none();
                };
                let targets = self.paste_targets(id);
                return iced::clipboard::read()
                    .map(move |contents| Message::ClipboardRead(id, targets.clone(), contents));
            }
            Message::ClipboardRead(id, targets, contents) => {
                // A delayed clipboard read cannot create a paste confirmation
                // for a session that the user has already left.
                if !self.focused_pane_matches(id)
                    || targets != self.paste_targets(id)
                    || self.dialog.is_some()
                {
                    self.status =
                        "Paste cancelled because terminal focus or the active dialog changed."
                            .into();
                    return Task::none();
                }
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
                        self.dialog = Some(Dialog::PasteConfirm {
                            id,
                            targets: self.paste_targets(id),
                            text,
                            normalize_line_endings: false,
                        });
                        self.status =
                            "Multiline paste is waiting for explicit confirmation.".into();
                    }
                    PasteDecision::Block => {
                        self.status =
                            "Paste blocked by policy (multiline or NUL-containing payload).".into();
                    }
                }
            }
            Message::TogglePasteNormalization => {
                if let Some(Dialog::PasteConfirm {
                    normalize_line_endings,
                    ..
                }) = &mut self.dialog
                {
                    *normalize_line_endings = !*normalize_line_endings;
                }
            }
            Message::ConfirmPaste => {
                if let Some(Dialog::PasteConfirm {
                    id,
                    targets,
                    text,
                    normalize_line_endings,
                }) = self.dialog.take()
                {
                    let prepared = normalized_paste_text(&text, normalize_line_endings);
                    // Never downgrade safety when normalization is selected.
                    // Content was originally classified for confirmation, and
                    // the prepared bytes must also remain valid under policy.
                    if terminal_ux::classify_paste(self.paste_policy, &prepared)
                        == PasteDecision::Block
                    {
                        self.status = "Paste blocked by safety policy.".into();
                    } else if targets == self.paste_targets(id)
                        && self.send_to_terminal(id, prepared.into_bytes())
                    {
                        self.status = "Paste sent after explicit confirmation.".into();
                    } else {
                        self.status =
                            "Paste cancelled because that terminal is no longer active.".into();
                    }
                }
            }
            Message::TerminalSelectStart(pane_id, id, x, y) => {
                if !self.active_pane_matches(pane_id, id) {
                    return Task::none();
                }
                if let Some(tab) = self.tabs.get_mut(self.active) {
                    tab.focus = pane_id;
                }
                if self.command_terminal(
                    id,
                    terminal_core::BackendCommand::SelectStart(
                        terminal_core::SelectionType::Simple,
                        x,
                        y,
                    ),
                ) {
                    return self.defer_terminal_refresh(id);
                }
            }
            Message::PanePaste(pane, id) => {
                if self.active_pane_matches(pane, id) {
                    self.tabs[self.active].focus = pane;
                    return self.update(Message::RequestPaste);
                }
            }
            Message::SelectionFinished(id, x, y, copy) => {
                let task = self.update(Message::TerminalSelectUpdate(id, x, y));
                if copy && self.focused_pane_matches(id) {
                    return Task::batch([task, self.update(Message::CopySelection(id))]);
                }
                return task;
            }
            Message::TerminalSelectUpdate(id, x, y) => {
                if !self.active_terminal_matches(id) {
                    return Task::none();
                }
                if self.command_terminal(id, terminal_core::BackendCommand::SelectUpdate(x, y)) {
                    return self.defer_terminal_refresh(id);
                }
            }
            Message::TerminalMouse(pane_id, id, button, modifiers, x, y, pressed) => {
                if self.local_navigation.contains(&id) || !self.active_pane_matches(pane_id, id) {
                    return Task::none();
                }
                if let Some(tab) = self.tabs.get_mut(self.active) {
                    tab.focus = pane_id;
                }
                self.command_terminal(
                    id,
                    terminal_core::BackendCommand::MouseReportAt(button, modifiers, x, y, pressed),
                );
            }
            Message::TerminalMouseWheel(pane_id, id, modifiers, x, y, lines) => {
                if !self.active_pane_matches(pane_id, id) {
                    return Task::none();
                }
                if let Some(tab) = self.tabs.get_mut(self.active) {
                    tab.focus = pane_id;
                }
                // While navigating locally, wheel movements must scroll
                // the local buffer even if the remote app has mouse mode on.
                if self.local_navigation.contains(&id) {
                    return self.update(Message::TerminalScroll(id, lines));
                }
                let button = if lines > 0 {
                    terminal_core::MouseButton::ScrollUp
                } else {
                    terminal_core::MouseButton::ScrollDown
                };
                for _ in 0..lines.unsigned_abs().min(8) {
                    self.command_terminal(
                        id,
                        terminal_core::BackendCommand::MouseReportAt(button, modifiers, x, y, true),
                    );
                }
            }
            Message::TerminalScroll(id, lines) => {
                if !self.active_terminal_matches(id) {
                    return Task::none();
                }
                if self.command_terminal(id, terminal_core::BackendCommand::Scroll(lines)) {
                    return self.defer_terminal_refresh(id);
                }
            }
            Message::CopySelection(id) => {
                if !self.active_terminal_matches(id) {
                    return Task::none();
                }
                if let Some(selection) = self.selected_terminal_text(id) {
                    self.status = "Terminal selection copied to clipboard.".into();
                    return iced::clipboard::write(selection);
                }
                self.status = "No terminal text is selected.".into();
            }
            Message::Reconnect(pane_id) => {
                self.tools.sync.set_armed(false);
                let profile = self
                    .tabs
                    .get(self.active)
                    .and_then(|tab| tab.panes.get(pane_id))
                    .map(|pane| (pane.profile.clone(), pane.sftp));

                if let Some((profile, sftp)) = profile {
                    let replacement = self.new_terminal_pane_kind(profile, sftp);
                    let connected = replacement.terminal.is_some();
                    let failed = replacement.error.is_some();
                    if let Some(tab) = self.tabs.get_mut(self.active)
                        && let Some(pane) = tab.panes.get_mut(pane_id)
                    {
                        *pane = replacement;
                        tab.focus = pane_id;
                        self.status = if connected {
                            "Reconnected through the production OpenSSH/PTY backend.".into()
                        } else if failed {
                            "Reconnect attempt failed; review the pane error.".into()
                        } else {
                            "Reconnect queued until the terminal event bridge is ready.".into()
                        };
                    }
                }
                self.prune_tool_panes();
            }
            Message::Split(axis) => {
                if self
                    .tabs
                    .get(self.active)
                    .is_some_and(|t| t.panes.len() >= 4)
                {
                    return Task::none();
                }
                self.tools.sync.set_armed(false);
                self.dialog = None;
                if let Some((profile, sftp)) = self
                    .tabs
                    .get(self.active)
                    .and_then(|tab| tab.panes.get(tab.focus))
                    .map(|p| (p.profile.clone(), p.sftp))
                {
                    let terminal = self.new_terminal_pane_kind(profile, sftp);
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
                self.tools.sync.set_armed(false);
                if let Some(tab) = self.tabs.get_mut(self.active) {
                    if tab.panes.len() == 1 {
                        self.dialog = Some(Dialog::Close(self.active));
                    } else if let Some((_, sibling)) = tab.panes.close(pane) {
                        tab.focus = sibling;
                    }
                }
                self.prune_tool_panes();
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
            Message::PublicKeyAuth(value) => self.form.public_key_auth = value,
            Message::PasswordAuth(value) => self.form.password_auth = value,
            Message::KeyboardInteractiveAuth(value) => self.form.keyboard_interactive_auth = value,
            Message::GssapiAuth(value) => self.form.gssapi_auth = value,
            Message::AgentForwarding(value) => self.form.agent_forwarding = value,
            Message::X11Forwarding(value) => self.form.x11_forwarding = value,
            Message::Compression(value) => self.form.compression = value,
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
                        self.refresh_profile_matches();
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
                self.start_configured_profile();
                self.pty_bridge = Some(sender.clone());
                let mut attempted = 0;
                let mut started = 0;
                for tab in &mut self.tabs {
                    for (_, pane) in tab.panes.iter_mut() {
                        if pane.terminal.is_none() && pane.error.is_none() && !pane.exited {
                            Self::start_terminal_pane(pane, &sender, self.ssh_config.as_deref());
                            attempted += 1;
                            if pane.terminal.is_some() {
                                started += 1;
                            }
                        }
                    }
                }
                self.status = if attempted == 0 {
                    "Terminal event bridge ready.".into()
                } else {
                    format!(
                        "Terminal bridge ready: {started} of {attempted} pending session(s) started."
                    )
                };
            }
            Message::PtyEvent(id, event) => {
                // Exit/Title/Bell/Wakeup from a closed or reconnected PTY can
                // remain queued briefly after its pane was removed. They must
                // never mutate a new session's UI or queue an orphan redraw.
                if !self.contains_terminal_pane(id) {
                    return Task::none();
                }
                let wakeup = matches!(&event, terminal_core::PtyEvent::Wakeup);
                let exited = matches!(
                    &event,
                    terminal_core::PtyEvent::Exit | terminal_core::PtyEvent::ChildExit(_)
                );
                match &event {
                    terminal_core::PtyEvent::Title(title) => {
                        self.update_terminal_title(id, sanitize_terminal_title(title));
                    }
                    terminal_core::PtyEvent::ResetTitle => {
                        self.update_terminal_title(id, None);
                    }
                    terminal_core::PtyEvent::Bell => {
                        self.status = format!("Terminal {id} rang the bell.");
                    }
                    // Remote OSC clipboard requests stay isolated from the host clipboard.
                    terminal_core::PtyEvent::ClipboardStore(_, _)
                    | terminal_core::PtyEvent::ClipboardLoad(_, _) => {}
                    _ => {}
                }
                if wakeup || exited {
                    self.queue_terminal_refresh(id, exited);
                }
                if exited {
                    self.status = format!("Terminal {id} exited.");
                }
                if wakeup || exited {
                    return self.schedule_terminal_frame();
                }
            }
            Message::FrameBridgeReady(sender) => {
                self.frame_bridge = Some(sender);
            }
            Message::TerminalFrame => {
                self.terminal_frame_scheduled = false;
                if !self.tabs.is_empty() {
                    self.refresh_workspace_displays(self.active);
                    let logged: Vec<_> = self
                        .tabs
                        .iter()
                        .enumerate()
                        .filter(|(index, t)| {
                            *index != self.active
                                && t.panes
                                    .iter()
                                    .any(|(_, p)| self.tools.logs.contains_key(&p.id))
                        })
                        .map(|(index, _)| index)
                        .collect();
                    for index in logged {
                        self.refresh_workspace_displays(index);
                    }
                }
                self.refresh_visible_history();
                return self.capture_session_logs();
            }
            Message::TerminalResized(id, size) => self.resize_terminal(id, size),
            Message::Event(iced::Event::Window(iced::window::Event::FileDropped(path))) => {
                if self.dialog.is_none()
                    && !self.focus_mode
                    && self.files_dock.is_some()
                    && !self.tabs.is_empty()
                {
                    self.files.selected_local = Some(path);
                    return self.request_upload();
                }
                self.status =
                    "Open Files for the target session before dropping an upload file.".into();
            }
            Message::Event(iced::Event::Keyboard(keyboard::Event::KeyPressed {
                key,
                modifiers,
                text,
                ..
            })) => {
                use keyboard::key::Named;

                if modifiers.command()
                    && key.as_ref() == keyboard::Key::Character(",")
                    && (self.dialog.is_none() || matches!(self.dialog, Some(Dialog::Tools)))
                {
                    return self.tool_update(tools::Action::Open(tools::Panel::Profiles));
                }
                if matches!(self.dialog, Some(Dialog::Tools))
                    && modifiers.command()
                    && let keyboard::Key::Character(value) = key.as_ref()
                    && let Ok(index) = value.parse::<usize>()
                    && (1..=8).contains(&index)
                {
                    return self.tool_update(tools::Action::Open(tools::Panel::ALL[index - 1]));
                }
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
                    keyboard::Key::Character("w")
                        if modifiers.command() && !self.tabs.is_empty() =>
                    {
                        return self.update(Message::AskClose(self.active));
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

                if let Some(id) = self.focused_terminal_id()
                    && self.local_navigation.contains(&id)
                {
                    if key.as_ref() == keyboard::Key::Named(Named::Escape) {
                        self.local_navigation.remove(&id);
                        self.status = "Remote input restored for focused pane.".into();
                    } else if let Some(lines) = local_navigation_scroll(&key, modifiers) {
                        return self.update(Message::TerminalScroll(id, lines));
                    }
                    // Never forward arbitrary keys in local-navigation mode.
                    return Task::none();
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
        if self.privacy_locked {
            // Do not render the sensitive workspace behind a translucent
            // modal: the entire view must be replaced, including tabs, host
            // labels, SFTP paths and terminal scrollback.
            return center(
                column![
                    text("Workspace hidden").size(30).color(FG),
                    text("SSH connections and transfers continue while this window is hidden.")
                        .size(15)
                        .color(MUTED),
                    text("This is an in-app privacy curtain, not an OS lock or encryption.")
                        .size(12)
                        .color(MUTED),
                    action("Unlock workspace", Message::TogglePrivacyLock).style(primary),
                    text("Shortcut: Ctrl/Cmd + Shift + L").size(12).color(BLUE),
                ]
                .spacing(18)
                .align_x(iced::Center),
            )
            .style(surface)
            .width(Fill)
            .height(Fill)
            .into();
        }
        let target = if self.dialog.is_none() && self.remote_editor.is_none() {
            self.focused_terminal_id()
                .filter(|id| !self.local_navigation.contains(id))
        } else {
            None
        };
        let cursor = self
            .ime_cursor
            .filter(|(id, _)| Some(*id) == target)
            .map(|(_, cursor)| cursor);
        terminal_ime::wrap(self.view_content(), target, cursor)
    }

    fn view_content(&self) -> Element<'_, Message> {
        let _slow = SlowIcedScope::start("view_layout");
        if self.focus_mode && !self.tabs.is_empty() {
            // Do not mutate the underlying dock/grid state. In focus mode the
            // workspace is reduced to the live terminal split panes, with an
            // always-visible exit action and synchronized-input warning.
            return container(
                column![
                    row![
                        text("FOCUS MODE · Alt+Enter to exit").size(12).color(BLUE),
                        text(&self.tabs[self.active].profile.name)
                            .size(12)
                            .color(MUTED),
                        space::horizontal(),
                        if self.tools.sync.armed() {
                            text("SYNC INPUT ARMED").size(12).color(DANGER)
                        } else {
                            text("").size(12)
                        },
                        action("Exit focus", Message::ToggleFocusMode),
                    ]
                    .spacing(12)
                    .align_y(iced::Center),
                    self.terminals(),
                ]
                .spacing(8)
                .height(Fill),
            )
            .padding(10)
            .width(Fill)
            .height(Fill)
            .style(surface)
            .into();
        }
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
                text("Native SSH workspace").size(12).color(MUTED),
                space::horizontal(),
                action(
                    "Tools",
                    Message::Tool(tools::Action::Open(tools::Panel::Profiles))
                ),
                action("Commands", Message::Commands),
                action("Lock", Message::TogglePrivacyLock),
                action("Focus", Message::ToggleFocusMode),
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
                text(if self.tools.sync.armed() {
                    format!("SYNC ARMED · {} panes", self.tools.sync.selected_count())
                } else {
                    String::new()
                })
                .size(12)
                .color(DANGER),
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
                // Enter belongs to the search box, never to the active SSH PTY.
                .on_submit(Message::Search(self.query.clone()))
                .padding(10),
            space::vertical().height(6),
        ]
        .spacing(12);

        let matches = self.profile_matches.len();
        let mut current_group = String::new();
        let mut count = 0;
        for (index, profile) in self
            .profile_matches
            .iter()
            .take(self.profile_visible)
            .filter_map(|&index| self.profiles.get(index).map(|profile| (index, profile)))
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
        } else if count < matches {
            list = list.push(
                action(
                    format!("Show more sessions ({count} of {matches})"),
                    Message::ShowMoreProfiles,
                )
                .width(Fill),
            );
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
            let label = self
                .tools
                .tab_labels
                .get(&tab.id)
                .copied()
                .unwrap_or_default();
            let color = match label {
                crate::tab_management::TabVisualLabel::None => MUTED,
                crate::tab_management::TabVisualLabel::Blue => BLUE,
                crate::tab_management::TabVisualLabel::Green => GREEN,
                crate::tab_management::TabVisualLabel::Amber => Color::from_rgb8(245, 190, 80),
                crate::tab_management::TabVisualLabel::Red => DANGER,
            };
            tabs = tabs.push(
                container(
                    row![
                        text(label.marker()).color(color),
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
            action(
                if self
                    .focused_terminal_id()
                    .is_some_and(|id| self.local_navigation.contains(&id))
                {
                    "LOCAL NAV"
                } else {
                    "REMOTE INPUT"
                },
                Message::ToggleLocalNavigation
            ),
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
                    text(if self.local_navigation.contains(&pane.id) {
                        "LOCAL NAV"
                    } else if focused {
                        "FOCUSED"
                    } else {
                        "SSH"
                    })
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
                    .unwrap_or_else(terminal_core::TerminalMode::empty);
                canvas(TerminalCanvas {
                    focused: self.focused_terminal_id() == Some(pane.id)
                        && self.dialog.is_none()
                        && self.remote_editor.is_none(),
                    pane: id,
                    id: pane.id,
                    generation: pane.display_generation,
                    terminal_mode,
                    snapshot,
                    appearance: self
                        .tools
                        .appearance
                        .effective_for(&session_profile_key(&pane.profile)),
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
            for entry in self
                .files
                .local_entries
                .iter()
                .take(self.files.local_visible)
            {
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
            if self.files.local_visible < self.files.local_entries.len() {
                local_list = local_list.push(
                    action(
                        format!(
                            "Show more local files ({} of {})",
                            self.files.local_visible,
                            self.files.local_entries.len(),
                        ),
                        Message::FilesShowMoreLocal,
                    )
                    .width(Fill),
                );
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
            for entry in self
                .files
                .remote_entries
                .iter()
                .take(self.files.remote_visible)
            {
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
            if self.files.remote_visible < self.files.remote_entries.len() {
                remote_list = remote_list.push(
                    action(
                        format!(
                            "Show more remote files ({} of {})",
                            self.files.remote_visible,
                            self.files.remote_entries.len(),
                        ),
                        Message::FilesShowMoreRemote,
                    )
                    .width(Fill),
                );
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
                    // Rendering must not wait on an in-flight SFTP save.
                    let dirty = editor.dirty;
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
            Dialog::Tools => self.tools_view(),
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
                                text("Authentication · Inherit follows SSH config defaults")
                                    .size(11)
                                    .color(MUTED),
                                policy_field("Public-key authentication", self.form.public_key_auth, Message::PublicKeyAuth),
                                policy_field("Password authentication", self.form.password_auth, Message::PasswordAuth),
                                policy_field("Keyboard-interactive authentication", self.form.keyboard_interactive_auth, Message::KeyboardInteractiveAuth),
                                policy_field("Kerberos / GSSAPI authentication", self.form.gssapi_auth, Message::GssapiAuth),
                                text("GSSAPI requires Kerberos tickets and a capable system OpenSSH; runtime-verified against an isolated Linux realm, not yet verified on Windows/macOS.")
                                    .size(11)
                                    .color(MUTED),
                                text("Session behavior · inherited unless explicitly overridden")
                                    .size(11)
                                    .color(MUTED),
                                policy_field("SSH agent forwarding", self.form.agent_forwarding, Message::AgentForwarding),
                                policy_field("X11 forwarding", self.form.x11_forwarding, Message::X11Forwarding),
                                policy_field("Compression", self.form.compression, Message::Compression),
                                text("Only enable agent or X11 forwarding for remote hosts you trust; agent forwarding exposes signing capability to that host.")
                                    .size(11)
                                    .color(MUTED),
                                text("X11 requires a local X server/DISPLAY and remote X11 support; it has not been verified in Windows/macOS native CI.")
                                    .size(11)
                                    .color(MUTED),
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
                        action("Save only", Message::Tool(tools::Action::SaveDraft)),
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
                    text_editor(&self.snippet_content)
                        .placeholder("Snippet body (non-secret remote text)")
                        .on_action(Message::SnippetEdit)
                        .height(100)
                        .padding(9),
                    row![action("Save snippet", Message::SaveSnippet), action("Clear draft", Message::CancelSnippetEdit)].spacing(6),
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
                            action("Edit", Message::EditSnippet(snippet.name.clone())),
                            action("Delete", Message::DeleteSnippet(snippet.name.clone())),
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
                            text_editor(&self.command_content)
                                .placeholder("Remote-shell text to stage")
                                .on_action(Message::CommandEdit)
                                .height(120)
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
            Dialog::PasteConfirm {
                text: paste,
                normalize_line_endings,
                ..
            } => {
                let prepared = normalized_paste_text(paste, *normalize_line_endings);
                let preview = paste_preview_text(&prepared, 4000);
                let line_count = prepared.lines().count().max(1);
                column![
                    text("Confirm terminal paste").size(24),
                    text(format!(
                        "Paste {} bytes across approximately {} line(s)?",
                        prepared.len(),
                        line_count
                    )),
                    text("Review carefully. Multiline terminal pastes can execute several commands immediately.")
                        .size(13)
                        .color(MUTED),
                    action(
                        if *normalize_line_endings {
                            "Line endings: normalize CRLF / CR to LF (enabled)"
                        } else {
                            "Line endings: preserve original bytes (default)"
                        },
                        Message::TogglePasteNormalization
                    ),
                    text("The preview below shows the exact prepared text; nonprinting controls are escaped for review.")
                        .size(12)
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
                text("Inspirum Terminal · Iced").size(24),
                text("Inspirum Terminal uses Iced for the desktop interface and the shared OpenSSH/PTY backend for terminal sessions.\n\nTools includes profiles and startup, tabs and saved split workspaces, synchronized input, history and logging, terminal appearance and pointer preferences, SSH trust and multiplexing, forwarding, tmux, SCP, snippets and sanitized support reports.\n\nThe Files pane provides graphical SFTP transfers and remote editing. Clipboard and command sends retain explicit confirmation and terminal identity checks.")
                    .color(MUTED),
                action("Back to workspace", Message::CloseDialog).style(primary),
            ]
            .spacing(20)
            .into(),
        };

        container(scrollable(body))
            .width(if matches!(dialog, Dialog::Tools) {
                880
            } else {
                600
            })
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

fn policy_field<'a>(
    label: &'a str,
    value: Option<bool>,
    message: fn(Option<bool>) -> Message,
) -> Element<'a, Message> {
    column![
        text(label).size(12).color(MUTED),
        row![
            action("Inherit", message(None)).style(if value.is_none() {
                selected_button
            } else {
                quiet
            }),
            action("Enable", message(Some(true))).style(if value == Some(true) {
                selected_button
            } else {
                quiet
            }),
            action("Disable", message(Some(false))).style(if value == Some(false) {
                selected_button
            } else {
                quiet
            }),
        ]
        .spacing(8),
    ]
    .spacing(6)
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
    #[test]
    fn remote_parent_stays_bounded() {
        assert_eq!(super::remote_parent("/a/b"), "/a");
        assert_eq!(super::remote_parent("/a"), "/");
        assert_eq!(super::remote_parent("/"), "/");
        assert_eq!(super::remote_parent("///"), "/");
        assert_eq!(super::remote_parent(""), ".");
        assert_eq!(super::remote_parent("a"), ".");
        assert_eq!(super::remote_parent("."), ".");
    }

    #[test]
    fn corrupt_profile_storage_survives_form_save_and_confirmed_replacement() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("profiles.json");
        std::fs::write(&path, b"broken").unwrap();
        let mut app = super::App::boot(path.clone(), None);
        assert!(!app.profiles_writable);
        app.form.host = "server.example".into();
        let _ = app.update(super::Message::Submit);
        assert!(app.form.error.is_some());
        let _ = app.tool_update(super::tools::Action::SaveDraft);
        app.tools.confirm = Some(super::tools::Confirm::ReplaceProfiles(
            dir.path().join("import.json"),
        ));
        let _ = app.tool_update(super::tools::Action::Confirm);
        let _ = app.view();
        assert_eq!(std::fs::read(path).unwrap(), b"broken");
        assert!(!app.tools.busy);
    }

    use super::*;

    #[test]
    fn saved_session_sidebar_pages_reset_after_search_changes() {
        let path = std::env::temp_dir().join(format!(
            "inspirum-iced-profile-page-{}-nonexistent.json",
            std::process::id()
        ));
        let mut app = App::boot(path, None);
        app.profiles = (0..250)
            .map(|index| Session {
                name: format!("server-{index}"),
                host: format!("host-{index}.example.invalid"),
                ..Session::default()
            })
            .collect();
        app.refresh_profile_matches();

        assert_eq!(app.profile_matches.len(), 250);
        assert_eq!(app.profile_visible, PROFILE_PAGE_SIZE);
        let _ = app.update(Message::ShowMoreProfiles);
        assert_eq!(app.profile_visible, PROFILE_PAGE_SIZE * 2);
        let _ = app.update(Message::ShowMoreProfiles);
        assert_eq!(app.profile_visible, 250);
        let _ = app.update(Message::ShowMoreProfiles);
        assert_eq!(app.profile_visible, 250);

        let _ = app.update(Message::Search("host-2".into()));
        assert_eq!(app.profile_visible, PROFILE_PAGE_SIZE);
        assert_eq!(app.profile_matches.len(), 61);
        assert_eq!(next_profile_page(usize::MAX, 250), 250);
    }

    #[test]
    fn privacy_lock_keeps_sessions_but_blocks_deferred_input() {
        let path = std::env::temp_dir().join(format!(
            "inspirum-iced-privacy-lock-{}-missing.json",
            std::process::id()
        ));
        let mut app = App::boot(path, None);
        let profile = Session {
            name: "masked-session".into(),
            host: "example.invalid".into(),
            ..Session::default()
        };
        let pane = app.new_terminal_pane(profile.clone());
        app.tabs.push(Workspace::new(profile, pane));
        assert!(!app.privacy_locked);

        let _ = app.update(Message::TogglePrivacyLock);
        assert!(app.privacy_locked);
        assert_eq!(app.tabs.len(), 1);
        let locked_status = app.status.clone();
        let _ = app.update(Message::RequestPaste);
        let _ = app.update(Message::CommandSend);
        let _ = app.update(Message::Tool(tools::Action::SendCommand));
        let _ = app.update(Message::Tool(tools::Action::Confirm));
        assert_eq!(app.status, locked_status);
        assert_eq!(app.tabs.len(), 1);

        let _ = app.update(Message::TogglePrivacyLock);
        assert!(!app.privacy_locked);
        assert_eq!(app.tabs.len(), 1);
    }

    #[test]
    fn focus_mode_is_presentation_only_and_privacy_lock_wins() {
        let path = std::env::temp_dir().join(format!(
            "inspirum-focus-mode-{}-missing.json",
            std::process::id()
        ));
        let mut app = App::boot(path, None);
        let _ = app.update(Message::ToggleFocusMode);
        assert!(!app.focus_mode);
        let profile = Session {
            name: "focus test".into(),
            host: "example.invalid".into(),
            ..Session::default()
        };
        let pane = app.new_terminal_pane(profile.clone());
        let id = pane.id;
        app.tabs.push(Workspace::new(profile, pane));
        let _ = app.update(Message::ToggleFocusMode);
        assert!(app.focus_mode);
        assert!(app.focused_pane_matches(id));
        assert_eq!(app.tabs.len(), 1);
        let _ = app.update(Message::TogglePrivacyLock);
        assert!(app.privacy_locked);
        let _ = app.update(Message::ToggleFocusMode);
        assert!(app.focus_mode, "a hidden workspace must not change mode");
        let _ = app.update(Message::TogglePrivacyLock);
        let _ = app.update(Message::ToggleFocusMode);
        assert!(!app.focus_mode);
        assert!(app.focused_pane_matches(id));
    }

    #[test]
    fn local_navigation_is_isolated_per_pane_and_blocks_terminal_writes() {
        let path = std::env::temp_dir().join(format!(
            "inspirum-local-navigation-{}-missing.json",
            std::process::id()
        ));
        let mut app = App::boot(path, None);
        let profile = Session {
            name: "navigation test".into(),
            host: "example.invalid".into(),
            ..Session::default()
        };
        let pane = app.new_terminal_pane(profile.clone());
        let id = pane.id;
        app.tabs.push(Workspace::new(profile, pane));
        let _ = app.update(Message::ToggleLocalNavigation);
        assert!(app.local_navigation.contains(&id));
        assert!(
            !app.command_terminal(id, terminal_core::BackendCommand::Write(b"danger".to_vec()))
        );
        assert!(!app.command_terminal(
            id,
            terminal_core::BackendCommand::MouseReportAt(
                terminal_core::MouseButton::LeftButton,
                terminal_core::MouseModifiers {
                    shift: false,
                    alt: false,
                    command: false,
                },
                0.0,
                0.0,
                true,
            )
        ));
        let _ = app.update(Message::RequestPaste);
        assert!(app.status.contains("Switch to remote"));
        let _ = app.update(Message::TogglePrivacyLock);
        let _ = app.update(Message::ToggleLocalNavigation);
        assert!(app.local_navigation.contains(&id));
        let _ = app.update(Message::TogglePrivacyLock);
        let _ = app.update(Message::ToggleLocalNavigation);
        assert!(!app.local_navigation.contains(&id));
    }

    #[test]
    fn shift_enter_is_local_navigation_only() {
        use keyboard::key::Named;
        let enter = keyboard::Key::Named(Named::Enter);
        assert!(local_navigation_chord(&enter, keyboard::Modifiers::SHIFT));
        assert!(!local_navigation_chord(
            &enter,
            keyboard::Modifiers::default()
        ));
        assert!(!local_navigation_chord(
            &enter,
            keyboard::Modifiers::SHIFT | keyboard::Modifiers::CTRL
        ));
        assert_eq!(
            local_navigation_scroll(
                &keyboard::Key::Named(Named::ArrowUp),
                keyboard::Modifiers::default(),
            ),
            Some(1)
        );
        assert_eq!(
            local_navigation_scroll(
                &keyboard::Key::Named(Named::PageDown),
                keyboard::Modifiers::default(),
            ),
            Some(-16)
        );
    }

    #[test]
    fn vi_like_keys_scroll_only_in_local_navigation() {
        let plain = keyboard::Modifiers::default();
        let ctrl = keyboard::Modifiers::CTRL;
        for (key, mods, expected) in [
            (keyboard::Key::Character("j".into()), plain, Some(-1)),
            (keyboard::Key::Character("k".into()), plain, Some(1)),
            (keyboard::Key::Character("g".into()), plain, Some(100_000)),
            (
                keyboard::Key::Character("G".into()),
                keyboard::Modifiers::SHIFT,
                Some(-100_000),
            ),
            (keyboard::Key::Character("d".into()), ctrl, Some(-8)),
            (keyboard::Key::Character("u".into()), ctrl, Some(8)),
            (keyboard::Key::Character("j".into()), ctrl, None),
            (keyboard::Key::Character("z".into()), plain, None),
        ] {
            assert_eq!(local_navigation_scroll(&key, mods), expected);
        }
    }

    #[test]
    fn alt_enter_toggles_focus_mode_without_stealing_plain_enter() {
        use keyboard::key::Named;
        let enter = keyboard::Key::Named(Named::Enter);
        assert!(focus_mode_chord(&enter, keyboard::Modifiers::ALT));
        assert!(!focus_mode_chord(&enter, keyboard::Modifiers::default()));
        assert!(!focus_mode_chord(
            &enter,
            keyboard::Modifiers::ALT | keyboard::Modifiers::CTRL
        ));
    }

    #[test]
    fn privacy_lock_chord_requires_command_and_shift() {
        let key = keyboard::Key::Character("l".into());
        let shift = keyboard::Modifiers::SHIFT;
        assert!(!privacy_lock_chord(&key, shift));
        assert!(!privacy_lock_chord(&key, keyboard::Modifiers::default()));
        assert!(!privacy_lock_chord(
            &keyboard::Key::Character("x".into()),
            keyboard::Modifiers::CTRL | shift,
        ));
    }

    #[test]
    fn iced_slow_trace_parsing_requires_positive_millisecond_threshold() {
        assert_eq!(
            parse_iced_trace_threshold("25"),
            Some(Duration::from_millis(25))
        );
        assert_eq!(
            parse_iced_trace_threshold(" 100 "),
            Some(Duration::from_millis(100))
        );
        assert_eq!(parse_iced_trace_threshold("0"), None);
        assert_eq!(parse_iced_trace_threshold(""), None);
        assert_eq!(parse_iced_trace_threshold("not-a-number"), None);
        assert_eq!(parse_iced_trace_threshold("-10"), None);
    }

    #[test]
    fn profile_search_cache_only_rebuilds_on_profile_or_query_changes() {
        let path = std::env::temp_dir().join(format!(
            "inspirum-iced-profile-cache-{}-nonexistent.json",
            std::process::id()
        ));
        let mut app = App::boot(path, None);
        app.profiles = vec![
            Session {
                name: "alpha".into(),
                host: "alpha.example.invalid".into(),
                ..Session::default()
            },
            Session {
                name: "beta".into(),
                host: "beta.example.invalid".into(),
                ..Session::default()
            },
        ];
        app.refresh_profile_matches();
        assert_eq!(app.profile_matches, vec![0, 1]);

        let _ = app.update(Message::Search("beta".into()));
        assert_eq!(app.profile_matches, vec![1]);

        // An unchanged search needs no full profile scan for every GUI redraw.
        let cached = app.profile_matches.clone();
        let _ = app.update(Message::ShowMoreProfiles);
        assert_eq!(app.profile_matches, cached);

        app.profiles.insert(
            0,
            Session {
                name: "beta-new".into(),
                host: "new.example.invalid".into(),
                ..Session::default()
            },
        );
        app.refresh_profile_matches();
        assert_eq!(app.profile_matches, vec![0, 2]);

        let _ = app.update(Message::Search(String::new()));
        assert_eq!(app.profile_matches, vec![0, 1, 2]);
    }

    #[test]
    fn upload_conflict_check_does_not_need_to_clone_remote_directory() {
        let entries = vec![
            sftp::RemoteEntry {
                name: "existing.txt".into(),
                is_dir: false,
                size: Some(10),
            },
            sftp::RemoteEntry {
                name: "folder".into(),
                is_dir: true,
                size: None,
            },
        ];
        assert!(upload_conflicts_with_remote_entries(
            std::path::Path::new("/tmp/existing.txt"),
            &entries
        ));
        assert!(!upload_conflicts_with_remote_entries(
            std::path::Path::new("/tmp/new.txt"),
            &entries
        ));
        assert!(upload_conflicts_with_remote_entries(
            std::path::Path::new("/tmp/folder"),
            &entries
        ));
    }

    #[test]
    fn delayed_clipboard_paste_is_cancelled_after_switching_tabs() {
        let path = std::env::temp_dir().join(format!(
            "inspirum-iced-stale-paste-{}-nonexistent.json",
            std::process::id()
        ));
        let mut app = App::boot(path, None);
        let profile = Session {
            name: "test".into(),
            host: "example.invalid".into(),
            ..Session::default()
        };
        let original = app.new_terminal_pane(profile.clone());
        let original_id = original.id;
        let next = app.new_terminal_pane(profile.clone());
        let next_id = next.id;
        app.tabs.push(Workspace::new(profile.clone(), original));
        app.tabs.push(Workspace::new(profile, next));
        app.active = 0;
        assert!(app.focused_pane_matches(original_id));
        app.active = 1;
        assert!(!app.focused_pane_matches(original_id));
        assert!(app.focused_pane_matches(next_id));

        // Neither an implicit one-line paste nor a multiline confirmation
        // may target a session after the user has switched away.
        let _ = app.update(Message::ClipboardRead(
            original_id,
            vec![original_id],
            Some("private clipboard text\nsecond line".into()),
        ));
        assert!(app.dialog.is_none());
        assert!(app.status.contains("cancelled"));
        assert!(!app.send_to_terminal(original_id, b"secret".to_vec()));

        // The user opened a connection form while the clipboard was loading.
        // The old paste response must not replace the active dialog.
        app.active = 0;
        app.dialog = Some(Dialog::Connection);
        let _ = app.update(Message::ClipboardRead(
            original_id,
            vec![original_id],
            Some("new dialog must remain visible".into()),
        ));
        assert!(matches!(app.dialog, Some(Dialog::Connection)));
        assert!(app.status.contains("cancelled"));
        app.active = 1;

        app.dialog = Some(Dialog::PasteConfirm {
            id: original_id,
            targets: vec![original_id],
            text: "line one\nline two".into(),
            normalize_line_endings: false,
        });
        let _ = app.update(Message::ConfirmPaste);
        assert!(app.dialog.is_none());
        assert!(app.status.contains("cancelled"));
    }

    #[test]
    fn blocking_file_worker_keeps_the_iced_executor_free() {
        use iced::futures::FutureExt as _;

        let (release, blocked) = std::sync::mpsc::channel::<()>();
        let mut operation = Box::pin(run_blocking_result(move || {
            blocked.recv().expect("worker release signal");
            Ok::<usize, String>(42)
        }));

        // Polling the Iced-side future must not wait for the blocking worker.
        assert!(operation.as_mut().now_or_never().is_none());
        release.send(()).expect("release blocking test worker");
        assert_eq!(iced::futures::executor::block_on(operation), Ok(42));
    }

    #[test]
    fn frame_timer_worker_ticks_only_on_requests_and_exits_cleanly() {
        let (requests, receiver) = std::sync::mpsc::channel();
        let (sender, ticks) = std::sync::mpsc::channel();
        let worker = thread::spawn(move || {
            frame_timer_worker(receiver, || sender.send(()).is_ok());
        });
        // The worker stays idle when no terminal frame has been requested.
        assert!(ticks.recv_timeout(Duration::from_millis(30)).is_err());
        requests.send(()).unwrap();
        assert!(ticks.recv_timeout(Duration::from_secs(2)).is_ok());
        assert!(ticks.try_recv().is_err());
        drop(requests);
        worker.join().expect("frame timer worker exited");
    }

    #[test]
    fn stale_canvas_events_cannot_refocus_a_different_session() {
        let path = std::env::temp_dir().join(format!(
            "inspirum-iced-stale-canvas-{}-nonexistent.json",
            std::process::id()
        ));
        let mut app = App::boot(path, None);
        let profile = Session {
            name: "test".into(),
            host: "example.invalid".into(),
            ..Session::default()
        };
        let original = app.new_terminal_pane(profile.clone());
        let original_id = original.id;
        app.tabs.push(Workspace::new(profile.clone(), original));
        let (old_pane, _) = app.tabs[0].panes.iter().next().unwrap();
        let old_pane = *old_pane;

        let next = app.new_terminal_pane(profile.clone());
        let next_id = next.id;
        app.tabs.push(Workspace::new(profile, next));
        app.active = 1;
        let focus_before = app.tabs[1].focus;

        assert!(!app.active_terminal_matches(original_id));
        assert!(!app.active_pane_matches(old_pane, original_id));
        let _ = app.update(Message::TerminalSelectStart(
            old_pane,
            original_id,
            3.0,
            4.0,
        ));
        assert_eq!(app.tabs[1].focus, focus_before);
        let _ = app.update(Message::TerminalSelectUpdate(original_id, 5.0, 6.0));
        let _ = app.update(Message::TerminalScroll(original_id, -3));
        assert_eq!(app.tabs[1].focus, focus_before);
        assert!(app.active_terminal_matches(next_id));

        // Reconnect replaces the terminal ID even if its pane handle is reused.
        let new_id = app.next_terminal_id;
        let replacement = app.new_terminal_pane(app.tabs[1].profile.clone());
        assert_eq!(replacement.id, new_id);
        *app.tabs[1].panes.get_mut(focus_before).unwrap() = replacement;
        assert!(!app.active_pane_matches(focus_before, next_id));
        assert!(app.active_pane_matches(focus_before, new_id));
    }

    #[test]
    fn sftp_file_lists_render_in_bounded_pages() {
        let files = FilesState::new();
        assert_eq!(files.local_visible, FILES_PAGE_SIZE);
        assert_eq!(files.remote_visible, FILES_PAGE_SIZE);
        assert_eq!(next_file_page(FILES_PAGE_SIZE, 10_000), FILES_PAGE_SIZE * 2);
        assert_eq!(next_file_page(400, 450), 450);
        assert_eq!(next_file_page(usize::MAX, 1_000), 1_000);
    }

    #[test]
    fn active_session_guard_rejects_events_after_tab_switch() {
        let path = std::env::temp_dir().join(format!(
            "inspirum-iced-active-editor-{}-nonexistent.json",
            std::process::id()
        ));
        let mut app = App::boot(path, None);
        let first = Session {
            name: "first".into(),
            host: "first.example.invalid".into(),
            ..Session::default()
        };
        let second = Session {
            name: "second".into(),
            host: "second.example.invalid".into(),
            ..Session::default()
        };
        let first_key = session_profile_key(&first);
        let second_key = session_profile_key(&second);
        let first_terminal = app.new_terminal_pane(first.clone());
        let second_terminal = app.new_terminal_pane(second.clone());
        app.tabs.push(Workspace::new(first, first_terminal));
        app.tabs.push(Workspace::new(second, second_terminal));

        app.active = 0;
        assert!(app.active_session_matches(&first_key));
        assert!(!app.active_session_matches(&second_key));
        app.active = 1;
        assert!(!app.active_session_matches(&first_key));
        assert!(app.active_session_matches(&second_key));
    }

    #[test]
    fn stale_remote_editor_callbacks_cannot_modify_current_status() {
        let path = std::env::temp_dir().join(format!(
            "inspirum-iced-editor-generation-{}-nonexistent.json",
            std::process::id()
        ));
        let mut app = App::boot(path, None);
        app.remote_editor_open_generation = 4;
        app.status = "Current editor remains active".into();
        let original_status = app.status.clone();
        let _ = app.update(Message::RemoteEditorOpened {
            request_id: 3,
            session: Session::default(),
            session_key: "previous".into(),
            remote: "/tmp/old.txt".into(),
            result: Err("stale SFTP open failure".into()),
        });
        assert_eq!(app.status, original_status);
        let _ = app.update(Message::RemoteEditorSaved {
            editor_id: 12,
            result: Ok(SaveOutcome::Conflict),
        });
        assert_eq!(app.status, original_status);
    }

    #[test]
    fn early_ssh_open_waits_for_bridge_and_retries_once_with_existing_pane_id() {
        let path = std::env::temp_dir().join(format!(
            "inspirum-iced-pending-{}-nonexistent.json",
            std::process::id()
        ));
        let mut app = App::boot(path, None);
        let profile = Session {
            name: "invalid test host".into(),
            host: "-o".into(),
            ..Session::default()
        };
        let pane = app.new_terminal_pane(profile.clone());
        let id = pane.id;
        assert!(pane.terminal.is_none());
        assert!(pane.error.is_none());
        app.tabs.push(Workspace::new(profile, pane));

        // Resize received before the bridge comes up must remain attached to the
        // same pane when the backend is started.
        app.resize_terminal(id, iced::Size::new(800.0, 500.0));
        let size = app.tabs[0]
            .panes
            .iter()
            .next()
            .unwrap()
            .1
            .terminal_grid_size;
        assert!(size.is_some());

        let (sender, _receiver) = mpsc::unbounded();
        let _ = app.update(Message::PtyBridgeReady(sender.clone()));
        let pending = app.tabs[0].panes.iter().next().unwrap().1;
        assert_eq!(pending.id, id);
        assert_eq!(pending.terminal_grid_size, size);
        // Invalid host fails Session validation before invoking any SSH process.
        assert!(pending.error.is_some());

        let previous_error = pending.error.clone();
        let _ = app.update(Message::PtyBridgeReady(sender));
        let pane = app.tabs[0].panes.iter().next().unwrap().1;
        assert_eq!(pane.id, id);
        assert_eq!(pane.error, previous_error);
    }

    #[test]
    fn background_cache_ignores_text_generations_but_tracks_size_and_color() {
        let state = TerminalCanvasState::default();
        let black = [0, 0, 0];
        let navy = [12, 18, 30];
        assert!(state.background_changed(800.0, 600.0, black));
        assert!(!state.background_changed(800.0, 600.0, black));
        // This remains cached when PTY output advances the text generation.
        state.generation.set(42);
        assert!(!state.background_changed(800.0, 600.0, black));
        assert!(state.background_changed(801.0, 600.0, black));
        assert!(!state.background_changed(801.0, 600.0, black));
        assert!(state.background_changed(801.0, 600.0, navy));
    }

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
    fn iced_advanced_authentication_and_forwarding_policies_round_trip() {
        let mut original = Session {
            name: "secure".into(),
            host: "server.example".into(),
            ..Session::default()
        };
        original.ssh.public_key_auth = Some(true);
        original.ssh.password_auth = Some(false);
        original.ssh.keyboard_interactive_auth = Some(false);
        original.ssh.gssapi_auth = Some(true);
        original.ssh.agent_forwarding = Some(false);
        original.ssh.x11_forwarding = Some(true);
        original.ssh.compression = Some(true);
        original.ssh.gssapi_delegate_credentials = Some(false);

        let mut form = ConnectionForm::from_session(&original);
        assert!(form.advanced);
        assert_eq!(form.public_key_auth, Some(true));
        assert_eq!(form.password_auth, Some(false));
        form.agent_forwarding = Some(true);
        form.password_auth = None;
        let updated = form.session().expect("valid advanced SSH policy");
        assert_eq!(updated.ssh.public_key_auth, Some(true));
        assert_eq!(updated.ssh.password_auth, None);
        assert_eq!(updated.ssh.keyboard_interactive_auth, Some(false));
        assert_eq!(updated.ssh.gssapi_auth, Some(true));
        assert_eq!(updated.ssh.agent_forwarding, Some(true));
        assert_eq!(updated.ssh.x11_forwarding, Some(true));
        assert_eq!(updated.ssh.compression, Some(true));
        assert_eq!(updated.ssh.gssapi_delegate_credentials, Some(false));
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
    fn enhanced_paste_requires_explicit_normalization_and_escapes_controls() {
        let input = "first\r\nsecond\rthird\u{001b}[31m";
        assert_eq!(normalized_paste_text(input, false), input);
        assert_eq!(
            normalized_paste_text(input, true),
            "first\nsecond\nthird\u{001b}[31m"
        );
        assert_eq!(
            paste_preview_text(&normalized_paste_text(input, true), 4000),
            "first\nsecond\nthird\\u{001b}[31m"
        );
        assert_eq!(
            paste_preview_text("abcde", 3),
            "abc\n... preview truncated ..."
        );
        // A normalized multiline payload still requires confirmation.
        assert_eq!(
            terminal_ux::classify_paste(
                PastePolicy::ConfirmMultiline,
                &normalized_paste_text("one\r\ntwo", true),
            ),
            PasteDecision::Confirm
        );
        assert_eq!(
            terminal_ux::classify_paste(PastePolicy::ConfirmMultiline, "bad\0data"),
            PasteDecision::Block
        );
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
                terminal_core::TerminalMode::empty(),
            ),
            Some(b"\x1b[A".to_vec())
        );
        assert_eq!(
            terminal_key_bytes(
                &keyboard::Key::Named(Named::ArrowUp),
                none,
                None,
                terminal_core::TerminalMode::APP_CURSOR,
            ),
            Some(b"\x1bOA".to_vec())
        );

        let shift = keyboard::Modifiers::SHIFT;
        assert_eq!(
            terminal_key_bytes(
                &keyboard::Key::Named(Named::Tab),
                shift,
                None,
                terminal_core::TerminalMode::empty(),
            ),
            Some(b"\x1b[Z".to_vec())
        );

        let alt = keyboard::Modifiers::ALT;
        assert_eq!(
            terminal_key_bytes(
                &keyboard::Key::Character("x".into()),
                alt,
                Some("x"),
                terminal_core::TerminalMode::empty(),
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
                terminal_core::TerminalMode::APP_CURSOR,
            ),
            Some(b"\x1b[1;5D".to_vec())
        );
        assert_eq!(
            terminal_key_bytes(
                &keyboard::Key::Named(Named::ArrowRight),
                keyboard::Modifiers::ALT | keyboard::Modifiers::SHIFT,
                None,
                terminal_core::TerminalMode::empty(),
            ),
            Some(b"\x1b[1;4C".to_vec())
        );
        assert_eq!(
            terminal_key_bytes(
                &keyboard::Key::Named(Named::Home),
                keyboard::Modifiers::SHIFT,
                None,
                terminal_core::TerminalMode::empty(),
            ),
            Some(b"\x1b[1;2H".to_vec())
        );
    }

    #[test]
    fn terminal_keys_send_committed_cjk_text_without_preedit_bytes() {
        let none = keyboard::Modifiers::default();
        let key = keyboard::Key::Character("中".into());

        assert_eq!(
            terminal_key_bytes(
                &key,
                none,
                Some("中文"),
                terminal_core::TerminalMode::empty(),
            ),
            Some("中文".as_bytes().to_vec())
        );

        assert_eq!(
            terminal_key_bytes(&key, none, None, terminal_core::TerminalMode::empty(),),
            None
        );
    }

    #[test]
    fn selection_and_pty_output_share_one_deferred_display_frame() {
        let profiles_path = std::env::temp_dir().join(format!(
            "inspirum-iced-selection-batch-{}-missing.json",
            std::process::id()
        ));
        let mut app = App::boot(profiles_path, None);
        let profile = Session {
            name: "test selection".into(),
            host: "example.invalid".into(),
            ..Session::default()
        };
        let pane = app.new_terminal_pane(profile.clone());
        let id = pane.id;
        app.tabs.push(Workspace::new(profile, pane));
        app.refresh_workspace_displays(0);
        assert!(!app.terminal_frame_scheduled);
        assert!(!app.tabs[0].panes.iter().next().unwrap().1.display_dirty);

        let _ = app.defer_terminal_refresh(id);
        assert!(app.terminal_frame_scheduled);
        assert!(app.tabs[0].panes.iter().next().unwrap().1.display_dirty);
        let _ = app.defer_terminal_refresh(id);
        assert!(app.terminal_frame_scheduled);

        let _ = app.update(Message::PtyEvent(id, terminal_core::PtyEvent::Wakeup));
        assert!(app.terminal_frame_scheduled);

        let _ = app.update(Message::TerminalFrame);
        assert!(!app.terminal_frame_scheduled);
        assert!(!app.tabs[0].panes.iter().next().unwrap().1.display_dirty);
        let _ = app.defer_terminal_refresh(id);
        assert!(app.terminal_frame_scheduled);
    }

    #[test]
    fn pty_burst_schedules_only_one_deferred_frame_until_refresh() {
        let profiles_path = std::env::temp_dir().join(format!(
            "inspirum-iced-frame-test-{}-missing.json",
            std::process::id()
        ));
        let mut app = App::boot(profiles_path, None);
        let profile = Session {
            name: "test host".into(),
            host: "example.com".into(),
            ..Session::default()
        };
        let first = app.new_terminal_pane(profile.clone());
        let second = app.new_terminal_pane(profile.clone());
        let first_id = first.id;
        let second_id = second.id;
        app.tabs.push(Workspace::new(profile.clone(), first));
        app.tabs.push(Workspace::new(profile, second));
        assert!(!app.terminal_frame_scheduled);

        // Two open sessions wake in the same interval, but only one Iced
        // frame may be scheduled. No network connection is required.
        let _ = app.update(Message::PtyEvent(first_id, terminal_core::PtyEvent::Wakeup));
        assert!(app.terminal_frame_scheduled);
        let _ = app.update(Message::PtyEvent(
            second_id,
            terminal_core::PtyEvent::Wakeup,
        ));
        assert!(app.terminal_frame_scheduled);

        let _ = app.update(Message::TerminalFrame);
        assert!(!app.terminal_frame_scheduled);

        let _ = app.update(Message::PtyEvent(first_id, terminal_core::PtyEvent::Wakeup));
        assert!(app.terminal_frame_scheduled);
    }

    #[test]
    fn events_from_removed_terminal_panes_do_not_mutate_workspace() {
        let profiles_path = std::env::temp_dir().join(format!(
            "inspirum-iced-stale-event-{}-missing.json",
            std::process::id()
        ));
        let mut app = App::boot(profiles_path, None);
        let before = app.status.clone();
        let _ = app.update(Message::PtyEvent(777, terminal_core::PtyEvent::Bell));
        assert_eq!(app.status, before);

        let _ = app.update(Message::PtyEvent(777, terminal_core::PtyEvent::Wakeup));
        assert!(!app.terminal_frame_scheduled);
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
