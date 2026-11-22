//! Production Iced application shell.
//!
//! This module intentionally reuses the existing validated Session/profile model.
//! The frontend uses the live PTY backend and preserves
//! host-key, argv, paste, process-lifecycle, or transfer safeguards.
mod dock;
mod dock_view;
use dock_view::available_panels;
mod icons;
mod menu;
mod terminal_ime;
mod theme;
mod tools;
mod ui_state;

use crate::{
    ProxyAuth, ProxyKind, Session, SessionImportMode,
    command_palette::{self, PaletteItem, Snippet, SnippetLibrary},
    import_sessions, load_sessions,
    remote_edit::{self, SaveOutcome},
    save_session_edit, save_sessions, session_matches_query, session_profile_key,
    session_requires_forward_risk_ack, sftp, terminal,
    terminal_ux::{self, PasteDecision, PastePolicy},
    tunnels,
};
use iced::futures::{SinkExt, Stream, StreamExt, channel::mpsc};
use iced::widget::{
    Column, button, canvas, center, column, container, mouse_area, opaque, operation, pane_grid,
    progress_bar, responsive, row, scrollable, sensor, space, stack, text, text_editor, text_input,
};
use iced::{
    Border, Color, Element, Fill, Font, Length, Subscription, Task, Theme, event, font, keyboard,
    mouse,
};
use std::{
    cell::{Cell, RefCell},
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

/// Fixed accent for marks drawn on the terminal canvas (link/IME underlines).
/// The terminal keeps its own dark palette, independent of the chrome theme.
const TERMINAL_ACCENT: Color = Color::from_rgb8(123, 176, 255);

/// Stable widget id for the Find strip input. Opening the strip focuses it.
const FIND_INPUT_ID: &str = "find-query";

// Chrome colour lookups against the current WindTerm theme palette.
fn fg() -> Color {
    theme::palette().text
}
fn muted() -> Color {
    theme::palette().muted
}
fn blue() -> Color {
    theme::palette().accent
}
fn green() -> Color {
    theme::palette().success
}
fn danger() -> Color {
    theme::palette().danger
}

const TERMINAL_CELL_WIDTH: f32 = 8.4;
const TERMINAL_CELL_HEIGHT: f32 = 18.0;
const FILES_PAGE_SIZE: usize = 200;
const PROFILE_PAGE_SIZE: usize = 100;

// Enable on demand with INSPIRUM_ICED_TRACE_MS=25. There is no per-frame
// console output and no clock read when the setting is absent.
static ICED_TRACE_THRESHOLD: OnceLock<Option<Duration>> = OnceLock::new();
// Tracing is entirely opt-in and stores only terminal IDs and monotonic time.
// Click-to-canvas timing is NOT screen-present latency (GPU/OS not included).
static PENDING_TERMINAL_CLICKS: OnceLock<Mutex<HashMap<u64, Instant>>> = OnceLock::new();

fn trace_terminal_click_started(id: u64) {
    if iced_trace_threshold().is_none() {
        return;
    }
    if let Ok(mut clicks) = PENDING_TERMINAL_CLICKS
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
    {
        if clicks.len() > 16 {
            clicks.clear();
        }
        clicks.insert(id, Instant::now());
    }
}

fn trace_terminal_click_canvas_drawn(id: u64) {
    let Some(threshold) = iced_trace_threshold() else {
        return;
    };
    let started = PENDING_TERMINAL_CLICKS
        .get()
        .and_then(|map| map.lock().ok().and_then(|mut clicks| clicks.remove(&id)));
    if let Some(started) = started {
        let elapsed = started.elapsed();
        if elapsed >= threshold {
            eprintln!(
                "iced slow click_to_canvas_draw: {:.1} ms (threshold {} ms)",
                elapsed.as_secs_f64() * 1000.0,
                threshold.as_millis()
            );
        }
    }
}

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

/// Advisory only: system OpenSSH and host config remain authoritative.
/// An absent variable is not proof that a host or platform lacks the feature.
fn forwarding_preflight_hint(
    agent: Option<bool>,
    x11: Option<bool>,
    agent_socket_present: bool,
    display_present: bool,
) -> String {
    let mut notes = Vec::new();
    if agent == Some(true) && cfg!(unix) && !agent_socket_present {
        notes.push("Agent preflight: SSH_AUTH_SOCK is unset. Check ssh-agent/IdentityAgent if forwarding fails.");
    }
    if x11 == Some(true) && !display_present {
        notes.push(
            "X11 preflight: DISPLAY is unset. Start/configure a local X server before connecting.",
        );
    }
    // An environment variable does not prove that agent/X11 transport works.
    // Keep native-unverified workflows visible without blocking OpenSSH or
    // declaring every third-party agent/X server unsupported.
    if agent == Some(true) && cfg!(windows) {
        notes.push(
            "Windows agent forwarding is not native-runtime verified; check the OpenSSH agent service and remote server.",
        );
    }
    if x11 == Some(true) && (cfg!(target_os = "macos") || cfg!(windows)) {
        notes.push(
            "X11 forwarding on this platform is not native-runtime verified; a working local X server is required.",
        );
    }
    notes.join(" ")
}

fn free_type_chord(key: &keyboard::Key, modifiers: keyboard::Modifiers) -> bool {
    modifiers.command()
        && modifiers.shift()
        && matches!(key.as_ref(), keyboard::Key::Character("e" | "E"))
}

fn remote_keys_chord(key: &keyboard::Key, modifiers: keyboard::Modifiers) -> bool {
    modifiers.command()
        && modifiers.shift()
        && matches!(key.as_ref(), keyboard::Key::Character("m" | "M"))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RemoteCursorKey {
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    PageUp,
    PageDown,
}

// This is explicitly remote key input, not an attempt to reposition the
// remote application's cursor from a locally inferred screen coordinate.
// The remote shell or full-screen program decides what each key does.
fn remote_vim_key(key: &keyboard::Key, modifiers: keyboard::Modifiers) -> Option<RemoteCursorKey> {
    if modifiers.command() || modifiers.control() || modifiers.alt() {
        return None;
    }
    match key.as_ref() {
        keyboard::Key::Character("h") if !modifiers.shift() => Some(RemoteCursorKey::Left),
        keyboard::Key::Character("j") if !modifiers.shift() => Some(RemoteCursorKey::Down),
        keyboard::Key::Character("k") if !modifiers.shift() => Some(RemoteCursorKey::Up),
        keyboard::Key::Character("l") if !modifiers.shift() => Some(RemoteCursorKey::Right),
        keyboard::Key::Character("0") if !modifiers.shift() => Some(RemoteCursorKey::Home),
        keyboard::Key::Character("$") => Some(RemoteCursorKey::End),
        keyboard::Key::Character("u") if !modifiers.shift() => Some(RemoteCursorKey::PageUp),
        keyboard::Key::Character("d") if !modifiers.shift() => Some(RemoteCursorKey::PageDown),
        _ => None,
    }
}

fn remote_cursor_bytes(action: RemoteCursorKey, mode: terminal_core::TerminalMode) -> Vec<u8> {
    use keyboard::key::Named;
    let key = match action {
        RemoteCursorKey::Left => Named::ArrowLeft,
        RemoteCursorKey::Right => Named::ArrowRight,
        RemoteCursorKey::Up => Named::ArrowUp,
        RemoteCursorKey::Down => Named::ArrowDown,
        RemoteCursorKey::Home => Named::Home,
        RemoteCursorKey::End => Named::End,
        RemoteCursorKey::PageUp => Named::PageUp,
        RemoteCursorKey::PageDown => Named::PageDown,
    };
    terminal_key_bytes(
        &keyboard::Key::Named(key),
        keyboard::Modifiers::empty(),
        None,
        mode,
    )
    .expect("cursor keys always encode to remote PTY input")
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

/// Translate local-only navigation gestures to bounded scrollback movement.
/// Remote SSH input is never generated by this function. Large movements
/// are clamped by terminal_core's scrollback bounds, not sent to the PTY.
fn wheel_scroll_lines(delta: &mouse::ScrollDelta, cell_height: f32) -> Option<i32> {
    let lines = match delta {
        mouse::ScrollDelta::Lines { y, .. } => *y,
        mouse::ScrollDelta::Pixels { y, .. } => *y / cell_height,
    };
    let lines = lines.round() as i32;
    (lines != 0).then_some(lines)
}

fn local_navigation_scroll(key: &keyboard::Key, modifiers: keyboard::Modifiers) -> Option<i32> {
    use keyboard::key::Named;
    if modifiers.alt() || modifiers.macos_command() {
        return None;
    }
    match key.as_ref() {
        keyboard::Key::Named(Named::ArrowUp) => Some(1),
        keyboard::Key::Named(Named::ArrowDown) => Some(-1),
        keyboard::Key::Named(Named::PageUp) => Some(16),
        keyboard::Key::Named(Named::PageDown) => Some(-16),
        keyboard::Key::Named(Named::Home) => Some(32767),
        keyboard::Key::Named(Named::End) => Some(-32767),
        keyboard::Key::Character("j") if !modifiers.shift() && !modifiers.control() => Some(-1),
        keyboard::Key::Character("k") if !modifiers.shift() && !modifiers.control() => Some(1),
        keyboard::Key::Character("u") if modifiers.control() => Some(16),
        keyboard::Key::Character("d") if modifiers.control() => Some(-16),
        keyboard::Key::Character("g") if !modifiers.shift() && !modifiers.control() => Some(32767),
        keyboard::Key::Character("G") if !modifiers.control() => Some(-32767),
        _ => None,
    }
}

pub fn run(profiles_path: PathBuf, ssh_config: Option<PathBuf>) -> iced::Result {
    theme::init_from_env();
    iced::application(
        move || App::boot(profiles_path.clone(), ssh_config.clone()),
        App::update,
        App::view,
    )
    .title("Inspirum Terminal")
    .theme(|_: &App| theme::iced_theme())
    .settings(iced::Settings {
        default_font: Font::MONOSPACE,
        default_text_size: iced::Pixels(theme::metrics::UI_TEXT),
        ..Default::default()
    })
    .window(iced::window::Settings {
        size: iced::Size::new(1280.0, 800.0),
        min_size: Some(iced::Size::new(960.0, 640.0)),
        ..Default::default()
    })
    // OS close must take the same guarded Exit path as Session > Exit.
    .exit_on_close_request(false)
    .scale_factor(|app: &App| app.scale as f32)
    .subscription(|app: &App| {
        let drag = if app.dock_drag.is_some() {
            event::listen_with(|event, _, _| match event {
                iced::Event::Mouse(iced::mouse::Event::CursorMoved { position }) => {
                    Some(Message::DockDragMove(position))
                }
                iced::Event::Mouse(iced::mouse::Event::ButtonReleased(
                    iced::mouse::Button::Left,
                ))
                | iced::Event::Mouse(iced::mouse::Event::CursorLeft)
                | iced::Event::Window(iced::window::Event::Unfocused) => Some(Message::DockDragEnd),
                _ => None,
            })
        } else {
            Subscription::none()
        };
        Subscription::batch([
            drag,
            // Kept because MouseArea::on_move is skipped when a child captured
            // the event (`shell.is_event_captured()`). Drags over a capturing
            // widget still need `last_pointer`. Context-menu anchors do not
            // use this path: the root `on_move` in `view` publishes
            // PointerMoved as a widget message during CursorMoved, and that
            // message is applied before the later ButtonPressed's right-press
            // message. Subscription output from this listener is broadcast
            // only after that widget batch.
            event::listen_with(|event, _, _| match event {
                iced::Event::Mouse(iced::mouse::Event::CursorMoved { position }) => {
                    Some(Message::PointerMoved(position))
                }
                _ => None,
            }),
            // Delivered after widgets process the click, including text input focus.
            event::listen_with(|event, _, _| match event {
                iced::Event::Mouse(iced::mouse::Event::ButtonPressed(
                    iced::mouse::Button::Left,
                )) => Some(Message::FindFocusProbe),
                _ => None,
            }),
            iced::window::resize_events().map(|(_, size)| Message::WindowResized(size)),
            iced::window::close_requests().map(|_| Message::Exit),
            event::listen_with(|event, status, _| match &event {
                iced::Event::Keyboard(keyboard::Event::KeyPressed { key, modifiers, .. })
                    if privacy_lock_chord(key, *modifiers) =>
                {
                    Some(Message::TogglePrivacyLock)
                }
                iced::Event::Keyboard(keyboard::Event::KeyPressed { key, modifiers, .. })
                    if focus_mode_chord(key, *modifiers) =>
                {
                    Some(Message::KeyChord(KeyChord::FocusMode))
                }
                iced::Event::Keyboard(keyboard::Event::KeyPressed { key, modifiers, .. })
                    if free_type_chord(key, *modifiers) =>
                {
                    Some(Message::KeyChord(KeyChord::FreeType))
                }
                iced::Event::Keyboard(keyboard::Event::KeyPressed { key, modifiers, .. })
                    if remote_keys_chord(key, *modifiers) =>
                {
                    Some(Message::KeyChord(KeyChord::RemoteKeys))
                }
                iced::Event::Keyboard(keyboard::Event::KeyPressed { key, modifiers, .. })
                    if status == event::Status::Ignored
                        && local_navigation_chord(key, *modifiers) =>
                {
                    Some(Message::KeyChord(KeyChord::LocalNavigation))
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
                // Keyboard-only viewport and explorer operations stay behind
                // widget capture: text editors own their own keys.
                iced::Event::Keyboard(keyboard::Event::KeyPressed { key, modifiers, .. })
                    if status == event::Status::Ignored
                        && modifiers.command()
                        && modifiers.shift()
                        && matches!(key.as_ref(), keyboard::Key::Character("r" | "R")) =>
                {
                    Some(Message::KeyChord(KeyChord::SplitRight))
                }
                // Ctrl/Cmd+Shift+F is retained-history Find (terminal-ux). The
                // files dock moved to Ctrl/Cmd+Shift+O so the chords do not collide.
                // Ignored-only, like the other viewport chords: a focused text
                // field, including the Find input, owns its keys.
                iced::Event::Keyboard(keyboard::Event::KeyPressed { key, modifiers, .. })
                    if status == event::Status::Ignored
                        && modifiers.command()
                        && modifiers.shift()
                        && matches!(key.as_ref(), keyboard::Key::Character("f" | "F")) =>
                {
                    Some(Message::KeyChord(KeyChord::Find))
                }
                iced::Event::Keyboard(keyboard::Event::KeyPressed { key, modifiers, .. })
                    if status == event::Status::Ignored
                        && modifiers.command()
                        && modifiers.shift()
                        && matches!(key.as_ref(), keyboard::Key::Character("o" | "O")) =>
                {
                    Some(Message::KeyChord(KeyChord::ToggleFiles))
                }
                iced::Event::Keyboard(keyboard::Event::KeyPressed { key, modifiers, .. })
                    if status == event::Status::Ignored
                        && modifiers.command()
                        && modifiers.shift()
                        && matches!(key.as_ref(), keyboard::Key::Character("j" | "J")) =>
                {
                    Some(Message::KeyChord(KeyChord::AuxiliaryShell))
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
    auxiliary_shell: bool,
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
    permissions: String,
    owner: String,
    group: String,
    modified: Option<std::time::SystemTime>,
}

impl LocalFileEntry {
    fn from_path(path: PathBuf) -> Self {
        let metadata = std::fs::symlink_metadata(&path).ok();
        let is_dir = metadata.as_ref().is_some_and(std::fs::Metadata::is_dir);
        let size = metadata
            .as_ref()
            .filter(|metadata| metadata.is_file())
            .map(std::fs::Metadata::len);
        let name = path
            .file_name()
            .map(|value| value.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.to_string_lossy().into_owned());
        let permissions = metadata
            .as_ref()
            .map(local_permissions)
            .unwrap_or_else(|| "-".into());
        let (owner, group) = metadata.as_ref().map(local_owner).unwrap_or_default();
        let modified = metadata
            .as_ref()
            .and_then(|metadata| metadata.modified().ok());
        Self {
            path,
            name,
            is_dir,
            size,
            permissions,
            owner,
            group,
            modified,
        }
    }
}

#[cfg(unix)]
fn local_permissions(metadata: &std::fs::Metadata) -> String {
    use std::os::unix::fs::PermissionsExt;
    let mode = metadata.permissions().mode();
    let kind = if metadata.is_dir() {
        'd'
    } else if metadata.file_type().is_symlink() {
        'l'
    } else {
        '-'
    };
    unix_mode_string(kind, mode)
}

#[cfg(not(unix))]
fn local_permissions(metadata: &std::fs::Metadata) -> String {
    let kind = if metadata.is_dir() { "d" } else { "-" };
    let access = if metadata.permissions().readonly() {
        "r--"
    } else {
        "rw-"
    };
    format!("{kind}{access}")
}

#[cfg_attr(not(unix), allow(dead_code))]
fn unix_mode_string(kind: char, mode: u32) -> String {
    let mut out = String::with_capacity(10);
    out.push(kind);
    for shift in [6, 3, 0] {
        let bits = (mode >> shift) & 0o7;
        out.push(if bits & 0o4 != 0 { 'r' } else { '-' });
        out.push(if bits & 0o2 != 0 { 'w' } else { '-' });
        out.push(if bits & 0o1 != 0 { 'x' } else { '-' });
    }
    let special = [(0o4000, 2, 's'), (0o2000, 5, 's'), (0o1000, 8, 't')];
    for (flag, index, letter) in special {
        if mode & flag != 0 {
            let index = index + 1;
            let executable = out.as_bytes()[index] == b'x';
            let letter = if executable {
                letter
            } else {
                letter.to_ascii_uppercase()
            };
            out.replace_range(index..index + 1, &letter.to_string());
        }
    }
    out
}

/// Parses an `/etc/passwd`- or `/etc/group`-style file into an id -> name map.
#[cfg(unix)]
fn parse_id_names(contents: &str) -> std::collections::HashMap<u32, String> {
    contents
        .lines()
        .filter_map(|line| {
            let mut fields = line.split(':');
            let name = fields.next()?;
            let id = fields.nth(1)?.parse().ok()?;
            (!name.is_empty()).then(|| (id, name.to_owned()))
        })
        .collect()
}

#[cfg(unix)]
fn cached_id_names(
    cell: &'static std::sync::OnceLock<std::collections::HashMap<u32, String>>,
    path: &str,
) -> &'static std::collections::HashMap<u32, String> {
    cell.get_or_init(|| {
        std::fs::read_to_string(path)
            .map(|contents| parse_id_names(&contents))
            .unwrap_or_default()
    })
}

#[cfg(unix)]
fn local_owner(metadata: &std::fs::Metadata) -> (String, String) {
    use std::os::unix::fs::MetadataExt;
    static USERS: std::sync::OnceLock<std::collections::HashMap<u32, String>> =
        std::sync::OnceLock::new();
    static GROUPS: std::sync::OnceLock<std::collections::HashMap<u32, String>> =
        std::sync::OnceLock::new();
    let name = |map: &std::collections::HashMap<u32, String>, id: u32| {
        map.get(&id).cloned().unwrap_or_else(|| id.to_string())
    };
    (
        name(cached_id_names(&USERS, "/etc/passwd"), metadata.uid()),
        name(cached_id_names(&GROUPS, "/etc/group"), metadata.gid()),
    )
}

#[cfg(not(unix))]
fn local_owner(_metadata: &std::fs::Metadata) -> (String, String) {
    (String::new(), String::new())
}

fn format_local_time(time: Option<std::time::SystemTime>) -> String {
    time.map(|time| {
        chrono::DateTime::<chrono::Local>::from(time)
            .format("%Y/%m/%d %H:%M:%S")
            .to_string()
    })
    .unwrap_or_default()
}

/// WindTerm-style size: `1 Byte`, `18 Bytes`, `8.0 KB`, `139.3 MB`, `5.1 GB`.
fn windterm_size(bytes: u64) -> String {
    const UNITS: [(u64, &str); 4] = [
        (1 << 40, "TB"),
        (1 << 30, "GB"),
        (1 << 20, "MB"),
        (1 << 10, "KB"),
    ];
    UNITS
        .iter()
        .find(|(scale, _)| bytes >= *scale)
        .map(|(scale, unit)| format!("{:.1} {unit}", bytes as f64 / *scale as f64))
        .unwrap_or_else(|| {
            if bytes == 1 {
                "1 Byte".into()
            } else {
                format!("{bytes} Bytes")
            }
        })
}

/// WindTerm-style type column: `Directory`, `TOML File`, `BASH_HISTORY File`.
fn windterm_type(name: &str, is_dir: bool) -> String {
    if is_dir {
        return "Directory".into();
    }
    let kind = match name.rsplit_once('.') {
        Some((stem, extension)) if !stem.is_empty() && !extension.is_empty() => extension,
        Some(("", rest)) if !rest.is_empty() => rest,
        _ => return "File".into(),
    };
    format!("{} File", kind.to_ascii_uppercase())
}

/// Converts an `ls -l` time (`Mon DD HH:MM` or `Mon DD YYYY`) into
/// `YYYY/MM/DD HH:MM` / `YYYY/MM/DD`; falls back to the raw text.
fn windterm_remote_date(raw: &str, today: chrono::NaiveDate) -> String {
    use chrono::Datelike;
    let fields: Vec<&str> = raw.split_whitespace().collect();
    let parsed = (|| {
        let [month, day, last] = fields.as_slice() else {
            return None;
        };
        let month = chrono::NaiveDate::parse_from_str(&format!("2000 {month} 1"), "%Y %b %d")
            .ok()?
            .month();
        let day: u32 = day.parse().ok()?;
        if let Some((hour, minute)) = last.split_once(':') {
            let mut year = today.year();
            let date = chrono::NaiveDate::from_ymd_opt(year, month, day)?;
            if date > today {
                year -= 1;
            }
            Some(format!(
                "{year:04}/{month:02}/{day:02} {hour:0>2}:{minute:0>2}"
            ))
        } else {
            let year: i32 = last.parse().ok()?;
            chrono::NaiveDate::from_ymd_opt(year, month, day)?;
            Some(format!("{year:04}/{month:02}/{day:02}"))
        }
    })();
    parsed.unwrap_or_else(|| raw.to_owned())
}

fn expand_local_path(input: &str) -> Option<PathBuf> {
    let input = input.trim();
    if input.is_empty() {
        return None;
    }
    let home = || directories::BaseDirs::new().map(|dirs| dirs.home_dir().to_path_buf());
    if input == "~" {
        return home();
    }
    if let Some(rest) = input
        .strip_prefix("~/")
        .or_else(|| input.strip_prefix("~\\"))
    {
        return home().map(|home| home.join(rest));
    }
    Some(PathBuf::from(input))
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
    /// Indices into `local_entries` in display order. Rebuilt only when the
    /// listing or the Explorer sort changes, never during view construction.
    local_order: Vec<usize>,
    /// Indices into `remote_entries` in display order. Rebuilt only when the
    /// listing or the Filer sort changes, never during view construction.
    remote_order: Vec<usize>,
    /// Client-local date captured once when the current remote listing was
    /// stored. Drives both the Date Modified sort key and the displayed date.
    /// Year inference for `ls` dates that omit a year is an approximation
    /// against this date, not against the clock at each comparison.
    remote_sort_reference: chrono::NaiveDate,
    /// Cheap instrumentation incremented each time local display order is
    /// recomputed. Tests read this to prove view construction does not re-sort.
    local_sort_runs: u64,
    /// Cheap instrumentation incremented each time remote display order is
    /// recomputed. Tests read this to prove view construction does not re-sort.
    remote_sort_runs: u64,
    selected_local: Option<PathBuf>,
    selected_remote: Option<sftp::RemoteEntry>,
    name_input: String,
    local_path_input: String,
    remote_path_input: String,
    transfers: Vec<TransferJob>,
}

impl FilesState {
    fn new() -> Self {
        let local_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        Self {
            session_key: None,
            local_path_input: local_dir.to_string_lossy().into_owned(),
            remote_path_input: ".".into(),
            local_dir,
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
            local_order: Vec::new(),
            remote_order: Vec::new(),
            remote_sort_reference: chrono::Local::now().date_naive(),
            local_sort_runs: 0,
            remote_sort_runs: 0,
            selected_local: None,
            selected_remote: None,
            name_input: String::new(),
            transfers: Vec::new(),
        }
    }

    /// The visible date uses the same listing-time reference as the sort key,
    /// so the label cannot change year between repaints while the row keeps
    /// its sorted position.
    fn remote_modified_label(&self, entry: &sftp::RemoteEntry) -> String {
        entry
            .modified
            .as_deref()
            .map(|raw| windterm_remote_date(raw, self.remote_sort_reference))
            .unwrap_or_default()
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

const FILE_COL_NAME: f32 = 290.0;
const FILE_COL_TIME: f32 = 168.0;
const FILE_COL_TYPE: f32 = 132.0;
const FILE_COL_SIZE: f32 = 76.0;
const FILE_COL_PERMS: f32 = 106.0;
const FILE_COL_OWNER: f32 = 62.0;
const FILE_COL_GROUP: f32 = 80.0;
const FILE_CELL_PAD: u16 = 6;

struct FileListingCells {
    name: String,
    is_dir: bool,
    modified: String,
    kind: String,
    size: String,
    permissions: String,
    owner: String,
    group: String,
}

fn file_listing_columns() -> [(&'static str, f32, bool); 7] {
    [
        ("Name", FILE_COL_NAME, false),
        ("Date Modified", FILE_COL_TIME, false),
        ("Type", FILE_COL_TYPE, false),
        ("Size", FILE_COL_SIZE, true),
        ("Permissions", FILE_COL_PERMS, false),
        ("Owner", FILE_COL_OWNER, false),
        ("Group", FILE_COL_GROUP, false),
    ]
}

fn file_listing_table_width() -> f32 {
    file_listing_columns()
        .iter()
        .map(|(_, width, _)| width + 1.0)
        .sum()
}

fn file_cell<'a>(value: String, width: impl Into<Length>, right: bool) -> Element<'a, Message> {
    let label = text(value)
        .size(theme::metrics::UI_TEXT)
        .font(Font::MONOSPACE)
        .color(theme::palette().text_dock)
        .wrapping(text::Wrapping::None);
    let cell = container(label)
        .width(width)
        .height(Fill)
        .padding([0, FILE_CELL_PAD])
        .align_y(iced::Center)
        .clip(true);
    if right {
        cell.align_x(iced::Right).into()
    } else {
        cell.into()
    }
}

fn file_listing_header<'a>(
    table_width: f32,
    remote: bool,
    sort_column: FileSortColumn,
    ascending: bool,
) -> Element<'a, Message> {
    let mut header = row![];
    let columns = file_listing_columns();
    let last = columns.len() - 1;
    debug_assert_eq!(columns.len(), FileSortColumn::COLUMNS.len());
    for (index, ((label, width, right), column)) in
        columns.into_iter().zip(FileSortColumn::COLUMNS).enumerate()
    {
        let width = if index == last {
            Length::Fill
        } else {
            Length::Fixed(width)
        };
        let message = if remote {
            Message::FilesSortRemote(column)
        } else {
            Message::FilesSortLocal(column)
        };
        header = header
            .push(file_header_cell(
                label,
                width,
                right,
                column == sort_column,
                ascending,
                message,
            ))
            .push(
                container(text(""))
                    .width(1)
                    .height(Fill)
                    .style(file_divider),
            );
    }
    column![
        container(header.height(theme::metrics::TABLE_HEADER_HEIGHT))
            .width(table_width)
            .style(file_header),
        container(text(""))
            .width(table_width)
            .height(1)
            .style(file_divider),
    ]
    .into()
}

const FILE_SORT_MARK: f32 = 8.0;

/// Flat header cell. Same width, font and padding as the previous label; a
/// press sorts that column. The sorted column draws a canvas triangle on its
/// right edge (up = ascending).
fn file_header_cell<'a>(
    label: &'static str,
    width: Length,
    right: bool,
    sorted: bool,
    ascending: bool,
    message: Message,
) -> Element<'a, Message> {
    let label = text(label)
        .size(theme::metrics::UI_TEXT)
        .font(Font::MONOSPACE)
        .color(theme::palette().text_dock)
        .wrapping(text::Wrapping::None);
    let label = container(label)
        .width(Fill)
        .height(Fill)
        .align_y(iced::Center)
        .align_x(if right { iced::Right } else { iced::Left })
        .clip(true);
    let mut content = row![label].align_y(iced::Center).width(Fill).height(Fill);
    if sorted {
        let glyph = if ascending {
            icons::Icon::TriangleUp
        } else {
            icons::Icon::TriangleDown
        };
        content = content.push(icons::icon(
            glyph,
            FILE_SORT_MARK,
            theme::palette().text_dock,
        ));
    }
    button(content)
        .on_press(message)
        .padding(if sorted {
            iced::Padding {
                top: 0.0,
                right: 1.0,
                bottom: 0.0,
                left: f32::from(FILE_CELL_PAD),
            }
        } else {
            iced::Padding::from([0, FILE_CELL_PAD])
        })
        .width(width)
        .height(Fill)
        .clip(true)
        .style(file_header_button)
        .into()
}

fn file_header_button(_theme: &Theme, status: button::Status) -> button::Style {
    let palette = theme::palette();
    let background = match status {
        button::Status::Hovered | button::Status::Pressed => palette.hover,
        _ => palette.header,
    };
    button::Style {
        background: Some(background.into()),
        text_color: palette.text_dock,
        border: Border::default(),
        ..button::Style::default()
    }
}

/// Column a Filer or Explorer table is ordered by. Header order matches
/// [`file_listing_columns`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FileSortColumn {
    Name,
    DateModified,
    Type,
    Size,
    Permissions,
    Owner,
    Group,
}

impl FileSortColumn {
    const COLUMNS: [Self; 7] = [
        Self::Name,
        Self::DateModified,
        Self::Type,
        Self::Size,
        Self::Permissions,
        Self::Owner,
        Self::Group,
    ];
}

/// A new column starts ascending; clicking the active column reverses it.
fn next_file_sort(
    current: FileSortColumn,
    ascending: bool,
    clicked: FileSortColumn,
) -> (FileSortColumn, bool) {
    if current == clicked {
        (current, !ascending)
    } else {
        (clicked, true)
    }
}

/// Values a file-table row can be ordered by.
///
/// `ModifiedKey` is an `Ord` value derived from the timestamp the entry
/// already stores. Local rows use `SystemTime` and ignore the reference
/// date. Remote rows parse the raw `ls -l` date into
/// `(year, month, day, hour, minute)` against the client-local date captured
/// when the listing arrived, so Date Modified is chronological. The column
/// still displays the formatted date, not this key.
trait FileSortEntry {
    type ModifiedKey: Ord;

    fn file_is_dir(&self) -> bool;
    fn file_name(&self) -> &str;
    fn file_size(&self) -> Option<u64>;
    fn file_type_key(&self) -> String;
    fn file_permissions(&self) -> &str;
    fn file_owner(&self) -> &str;
    fn file_group(&self) -> &str;
    /// `reference` is the client-local date captured when a remote listing
    /// arrived. Local rows ignore it.
    fn file_modified_key(&self, reference: chrono::NaiveDate) -> Self::ModifiedKey;
}

impl FileSortEntry for LocalFileEntry {
    type ModifiedKey = Option<std::time::SystemTime>;

    fn file_is_dir(&self) -> bool {
        self.is_dir
    }

    fn file_name(&self) -> &str {
        &self.name
    }

    fn file_size(&self) -> Option<u64> {
        self.size
    }

    fn file_type_key(&self) -> String {
        windterm_type(&self.name, self.is_dir)
    }

    fn file_permissions(&self) -> &str {
        &self.permissions
    }

    fn file_owner(&self) -> &str {
        &self.owner
    }

    fn file_group(&self) -> &str {
        &self.group
    }

    fn file_modified_key(&self, _reference: chrono::NaiveDate) -> Self::ModifiedKey {
        self.modified
    }
}

impl FileSortEntry for sftp::RemoteEntry {
    type ModifiedKey = Option<(i32, u32, u32, u32, u32)>;

    fn file_is_dir(&self) -> bool {
        self.is_dir
    }

    fn file_name(&self) -> &str {
        &self.name
    }

    fn file_size(&self) -> Option<u64> {
        self.size
    }

    fn file_type_key(&self) -> String {
        windterm_type(&self.name, self.is_dir)
    }

    fn file_permissions(&self) -> &str {
        self.permissions.as_deref().unwrap_or("")
    }

    fn file_owner(&self) -> &str {
        self.owner.as_deref().unwrap_or("")
    }

    fn file_group(&self) -> &str {
        self.group.as_deref().unwrap_or("")
    }

    fn file_modified_key(&self, reference: chrono::NaiveDate) -> Self::ModifiedKey {
        self.modified
            .as_deref()
            .and_then(|raw| ls_modified_sort_key(raw, reference))
    }
}

impl<T: FileSortEntry + ?Sized> FileSortEntry for &T {
    type ModifiedKey = T::ModifiedKey;

    fn file_is_dir(&self) -> bool {
        T::file_is_dir(*self)
    }

    fn file_name(&self) -> &str {
        T::file_name(*self)
    }

    fn file_size(&self) -> Option<u64> {
        T::file_size(*self)
    }

    fn file_type_key(&self) -> String {
        T::file_type_key(*self)
    }

    fn file_permissions(&self) -> &str {
        T::file_permissions(*self)
    }

    fn file_owner(&self) -> &str {
        T::file_owner(*self)
    }

    fn file_group(&self) -> &str {
        T::file_group(*self)
    }

    fn file_modified_key(&self, reference: chrono::NaiveDate) -> Self::ModifiedKey {
        T::file_modified_key(*self, reference)
    }
}

fn cmp_ignore_case(left: &str, right: &str) -> std::cmp::Ordering {
    left.to_lowercase().cmp(&right.to_lowercase())
}

/// Directories stay before files in every sort. `ascending` applies only to
/// `column`; ties break by lowercase name and do not reverse.
///
/// `reference` is the client-local date captured when a remote listing
/// arrived. Local rows ignore it. Date Modified keys are computed once per
/// entry, not once per comparison.
/// Test-only helper that applies `file_display_order` in place.
#[cfg(test)]
fn sort_file_entries<T: FileSortEntry>(
    entries: &mut [T],
    column: FileSortColumn,
    ascending: bool,
    reference: chrono::NaiveDate,
) {
    let order = file_display_order(entries, column, ascending, reference);
    apply_index_order(entries, order);
}

/// Indices of `entries` in display order. Does not reorder `entries`.
fn file_display_order<T: FileSortEntry>(
    entries: &[T],
    column: FileSortColumn,
    ascending: bool,
    reference: chrono::NaiveDate,
) -> Vec<usize> {
    let mut order: Vec<usize> = (0..entries.len()).collect();
    match column {
        FileSortColumn::DateModified => {
            let keys: Vec<T::ModifiedKey> = entries
                .iter()
                .map(|entry| entry.file_modified_key(reference))
                .collect();
            order.sort_by(|&left, &right| {
                file_sort_cmp(
                    entries[left].file_is_dir(),
                    entries[right].file_is_dir(),
                    keys[left].cmp(&keys[right]),
                    entries[left].file_name(),
                    entries[right].file_name(),
                    ascending,
                )
            });
        }
        other => {
            order.sort_by(|&left, &right| {
                file_sort_cmp(
                    entries[left].file_is_dir(),
                    entries[right].file_is_dir(),
                    non_date_ordering(&entries[left], &entries[right], other),
                    entries[left].file_name(),
                    entries[right].file_name(),
                    ascending,
                )
            });
        }
    }
    order
}

fn file_sort_cmp(
    left_dir: bool,
    right_dir: bool,
    primary: std::cmp::Ordering,
    left_name: &str,
    right_name: &str,
    ascending: bool,
) -> std::cmp::Ordering {
    let directories = right_dir.cmp(&left_dir);
    if directories != std::cmp::Ordering::Equal {
        return directories;
    }
    let primary = if ascending {
        primary
    } else {
        primary.reverse()
    };
    primary.then_with(|| cmp_ignore_case(left_name, right_name))
}

fn non_date_ordering<T: FileSortEntry>(
    left: &T,
    right: &T,
    column: FileSortColumn,
) -> std::cmp::Ordering {
    match column {
        FileSortColumn::Name => cmp_ignore_case(left.file_name(), right.file_name()),
        FileSortColumn::Type => cmp_ignore_case(&left.file_type_key(), &right.file_type_key()),
        FileSortColumn::Size => left.file_size().cmp(&right.file_size()),
        FileSortColumn::Permissions => {
            cmp_ignore_case(left.file_permissions(), right.file_permissions())
        }
        FileSortColumn::Owner => cmp_ignore_case(left.file_owner(), right.file_owner()),
        FileSortColumn::Group => cmp_ignore_case(left.file_group(), right.file_group()),
        // Date Modified uses the cached-key path in `file_display_order`.
        FileSortColumn::DateModified => std::cmp::Ordering::Equal,
    }
}

/// `order[dest]` is the source index of the item that belongs at `dest`.
/// Test-only helper that applies `file_display_order` in place.
#[cfg(test)]
fn apply_index_order<T>(items: &mut [T], mut order: Vec<usize>) {
    let mut index = 0;
    while index < order.len() {
        if order[index] != index {
            let mut current = index;
            loop {
                let next = order[current];
                order[current] = current;
                if next == index {
                    break;
                }
                items.swap(current, next);
                current = next;
            }
        }
        index += 1;
    }
}

/// Chronological key for an `ls -l` date (`Mon D HH:MM` or `Mon D YYYY`).
///
/// `ls` omits the year for files modified within about the last six months.
/// Year inference for that form is an approximation based on the client's
/// local date captured when the listing arrived (`now`), not on the clock at
/// comparison time. That form uses `now`'s year unless the month and day are
/// still in the future, in which case it is the previous year. A `YYYY` form
/// has no time and sorts at 00:00. Unparseable text is `None`, which sorts
/// before every real timestamp; the caller still breaks those ties by name.
fn ls_modified_sort_key(raw: &str, now: chrono::NaiveDate) -> Option<(i32, u32, u32, u32, u32)> {
    use chrono::Datelike;
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let mut fields = raw.split_whitespace();
    let month_name = fields.next()?;
    let day = fields.next()?;
    let last = fields.next()?;
    if fields.next().is_some() {
        return None;
    }
    let month = MONTHS
        .iter()
        .position(|name| *name == month_name)
        .map(|index| index as u32 + 1)?;
    let day: u32 = day.parse().ok()?;
    let (year, hour, minute) = if let Some((hour, minute)) = last.split_once(':') {
        let hour: u32 = hour.parse().ok()?;
        let minute: u32 = minute.parse().ok()?;
        if hour > 23 || minute > 59 {
            return None;
        }
        let mut year = now.year();
        if chrono::NaiveDate::from_ymd_opt(year, month, day)? > now {
            year -= 1;
        }
        chrono::NaiveDate::from_ymd_opt(year, month, day)?;
        (year, hour, minute)
    } else {
        let year: i32 = last.parse().ok()?;
        chrono::NaiveDate::from_ymd_opt(year, month, day)?;
        (year, 0, 0)
    };
    Some((year, month, day, hour, minute))
}

fn file_listing_row<'a>(
    cells: FileListingCells,
    message: Message,
    selected: bool,
    table_width: f32,
    on_right_press: Option<Message>,
) -> Element<'a, Message> {
    let (glyph, color) = if cells.is_dir {
        let color = match theme::current() {
            theme::ChromeTheme::Dark => Color::from_rgb8(0xF6, 0xD5, 0x6F),
            _ => Color::from_rgb8(0x12, 0xBF, 0xE5),
        };
        (icons::Icon::Folder, color)
    } else {
        (icons::Icon::File, theme::palette().muted)
    };
    // The name stays one unbroken text in the main dock text colour: the
    // OCR-driven GUI acceptance tests click it.
    let name = row![
        icons::icon(glyph, 14.0, color),
        text(cells.name)
            .size(theme::metrics::UI_TEXT)
            .font(Font::MONOSPACE)
            .color(theme::palette().text_dock)
            .wrapping(text::Wrapping::None),
    ]
    .spacing(6)
    .align_y(iced::Center);
    let name = container(name)
        .width(FILE_COL_NAME + 1.0)
        .height(Fill)
        .padding([0, FILE_CELL_PAD])
        .align_y(iced::Center)
        .clip(true);
    let content = row![
        name,
        file_cell(cells.modified, FILE_COL_TIME + 1.0, false),
        file_cell(cells.kind, FILE_COL_TYPE + 1.0, false),
        file_cell(cells.size, FILE_COL_SIZE + 1.0, true),
        file_cell(cells.permissions, FILE_COL_PERMS + 1.0, false),
        file_cell(cells.owner, FILE_COL_OWNER + 1.0, false),
        file_cell(cells.group, Length::Fill, false),
    ]
    .height(theme::metrics::ROW_HEIGHT);
    let row = button(content)
        .on_press(message)
        .padding(0)
        .width(table_width)
        .style(if selected {
            file_row_selected
        } else {
            file_row
        });
    match on_right_press {
        Some(on_right_press) => mouse_area(row).on_right_press(on_right_press).into(),
        None => row.into(),
    }
}

/// Which file panel owns the open context menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FileContextSide {
    Remote,
    Local,
}

/// Right-click menu on a Filer or Explorer row.
#[derive(Clone, Debug)]
struct FileContextMenu {
    side: FileContextSide,
    is_dir: bool,
    /// Display/remote path used by Open. Files leave this unused.
    open_path: String,
    /// Original local path for Open. Avoids a lossy string round-trip.
    local_path: Option<PathBuf>,
    anchor: iced::Point,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FileContextAction {
    Open,
    Download,
    Upload,
    Edit,
    NewFolder,
    Rename,
    Delete,
    Refresh,
}

/// One right-click menu. File rows and session rows share this state so the
/// existing keyboard, IME, paste and chord guards keep covering it.
#[derive(Clone, Debug)]
enum ContextMenu {
    File(FileContextMenu),
    Session {
        index: usize,
        key: String,
        anchor: iced::Point,
    },
}

impl ContextMenu {
    fn anchor(&self) -> iced::Point {
        match self {
            ContextMenu::File(menu) => menu.anchor,
            ContextMenu::Session { anchor, .. } => *anchor,
        }
    }

    #[cfg(test)]
    fn file(&self) -> Option<&FileContextMenu> {
        match self {
            ContextMenu::File(menu) => Some(menu),
            ContextMenu::Session { .. } => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SessionContextAction {
    Open,
    Edit,
    NewSession,
    Import,
}

const FILE_CONTEXT_MENU_WIDTH: f32 = 200.0;
const FILE_CONTEXT_ITEM_HEIGHT: f32 = 24.0;

fn file_context_menu_height(item_count: usize) -> f32 {
    item_count as f32 * FILE_CONTEXT_ITEM_HEIGHT + 2.0
}

/// Visual popup size, including the 1px padding that holds the border.
/// The column itself stays [`FILE_CONTEXT_MENU_WIDTH`] by [`FILE_CONTEXT_ITEM_HEIGHT`].
fn context_menu_popup_size(item_count: usize) -> iced::Size {
    iced::Size::new(
        FILE_CONTEXT_MENU_WIDTH + 2.0,
        file_context_menu_height(item_count),
    )
}

/// Keep a file or session context menu inside the window: at least 2px from the
/// left and right, at or below the menu bar, and at least 2px above the status
/// bar. `window` is the logical size from `WindowResized`; Iced applies `scale`
/// on its own, so this does not divide by it. A menu larger than that region
/// pins to the region's top-left.
fn clamp_context_menu_anchor(
    anchor: iced::Point,
    window: iced::Size,
    menu: iced::Size,
) -> iced::Point {
    let min_x = 2.0;
    let min_y = theme::metrics::MENU_BAR_HEIGHT;
    let max_x = (window.width - 2.0 - menu.width).max(min_x);
    let max_y = (window.height - theme::metrics::STATUS_BAR_HEIGHT - 2.0 - menu.height).max(min_y);
    iced::Point::new(anchor.x.clamp(min_x, max_x), anchor.y.clamp(min_y, max_y))
}

/// Spec 5.2 items that already have a panel message. Open is directories only.
/// Edit is remote files only. Download is remote files only and Upload is
/// local files only: both are always rejected for directories, so they are
/// omitted rather than shown. Local rows have no edit message, and neither
/// side has an open-file message, so those items are omitted too.
fn file_context_entries(menu: &FileContextMenu) -> Vec<(&'static str, FileContextAction)> {
    let mut items = Vec::new();
    if menu.is_dir {
        items.push(("Open", FileContextAction::Open));
    }
    match menu.side {
        FileContextSide::Remote => {
            if !menu.is_dir {
                items.push(("Download", FileContextAction::Download));
                items.push(("Edit", FileContextAction::Edit));
            }
        }
        FileContextSide::Local => {
            if !menu.is_dir {
                items.push(("Upload", FileContextAction::Upload));
            }
        }
    }
    items.push(("New Folder", FileContextAction::NewFolder));
    items.push(("Rename", FileContextAction::Rename));
    items.push(("Delete", FileContextAction::Delete));
    items.push(("Refresh", FileContextAction::Refresh));
    items
}

fn file_context_action_message(
    menu: &FileContextMenu,
    action: FileContextAction,
) -> Option<Message> {
    match (menu.side, action) {
        (FileContextSide::Remote, FileContextAction::Open) if menu.is_dir => {
            Some(Message::FilesOpenRemote(menu.open_path.clone()))
        }
        (FileContextSide::Local, FileContextAction::Open) if menu.is_dir => {
            menu.local_path.clone().map(Message::FilesOpenLocal)
        }
        (_, FileContextAction::Open) => None,
        (FileContextSide::Remote, FileContextAction::Download) => {
            Some(Message::FilesRequestDownload)
        }
        (FileContextSide::Local, FileContextAction::Upload) => Some(Message::FilesRequestUpload),
        (FileContextSide::Remote, FileContextAction::Edit) if !menu.is_dir => {
            Some(Message::FilesRequestEditRemote)
        }
        (FileContextSide::Remote, FileContextAction::NewFolder) => {
            Some(Message::FilesRequestMkdirRemote)
        }
        (FileContextSide::Local, FileContextAction::NewFolder) => {
            Some(Message::FilesRequestMkdirLocal)
        }
        (FileContextSide::Remote, FileContextAction::Rename) => {
            Some(Message::FilesRequestRenameRemote)
        }
        (FileContextSide::Local, FileContextAction::Rename) => {
            Some(Message::FilesRequestRenameLocal)
        }
        (FileContextSide::Remote, FileContextAction::Delete) => {
            Some(Message::FilesRequestDeleteRemote)
        }
        (FileContextSide::Local, FileContextAction::Delete) => {
            Some(Message::FilesRequestDeleteLocal)
        }
        (_, FileContextAction::Refresh) => Some(Message::FilesRefresh),
        // Cross-side items (local Download, remote Upload) and Edit on a
        // directory or a local row are not offered. Drop a stale message.
        _ => None,
    }
}

/// Spec 5.1 items. The popup has no separator widget, so none is drawn
/// between Edit and New Session.
fn session_context_entries() -> Vec<(&'static str, SessionContextAction)> {
    vec![
        ("Open", SessionContextAction::Open),
        ("Edit", SessionContextAction::Edit),
        ("New Session", SessionContextAction::NewSession),
        ("Import", SessionContextAction::Import),
    ]
}

/// Surface shared with the menu-bar drop-downs. `palette.border` is only
/// 1.61:1 against the light list fill and 1.71:1 against the dark list fill, so
/// the outline disappears on the dark panel. `text_dock` is at least 9:1
/// against dock and list in both themes.
fn menu_popup_style(_: &Theme) -> container::Style {
    let p = theme::palette();
    container::Style {
        background: Some(p.list.into()),
        border: Border {
            color: p.text_dock,
            width: 1.0,
            radius: 0.0.into(),
        },
        ..Default::default()
    }
}

fn context_popup<'a>(items: Vec<(&'static str, Message)>) -> Element<'a, Message> {
    let mut rows = column![].width(FILE_CONTEXT_MENU_WIDTH);
    for (label, message) in items {
        rows = rows.push(
            button(
                text(label)
                    .size(theme::metrics::UI_TEXT)
                    .color(theme::palette().text),
            )
            .width(Fill)
            .height(FILE_CONTEXT_ITEM_HEIGHT)
            .padding([3, 10])
            .style(menu_item)
            .on_press(message),
        );
    }
    opaque(container(rows).padding(1).style(menu_popup_style))
}

/// Same stack pattern as `dock_menu_overlay`: a full-size press layer dismisses,
/// and padding anchors the flat menu at the pointer. Session rows use the same
/// popup; it has no separator row, so that menu is four items.
fn file_context_overlay<'a>(
    body: Element<'a, Message>,
    menu: &ContextMenu,
    window: iced::Size,
) -> Element<'a, Message> {
    let items: Vec<(&'static str, Message)> = match menu {
        ContextMenu::File(file) => file_context_entries(file)
            .into_iter()
            .map(|(label, action)| (label, Message::FileContextChoose(action)))
            .collect(),
        ContextMenu::Session { .. } => session_context_entries()
            .into_iter()
            .map(|(label, action)| (label, Message::SessionContextChoose(action)))
            .collect(),
    };
    let anchor =
        clamp_context_menu_anchor(menu.anchor(), window, context_menu_popup_size(items.len()));
    let popup = context_popup(items);
    stack![
        body,
        mouse_area(container(space::horizontal()).width(Fill).height(Fill))
            .on_press(Message::FileContextDismiss)
            .on_right_press(Message::FileContextDismiss),
        container(popup)
            .padding(iced::Padding {
                top: anchor.y,
                left: anchor.x,
                ..iced::Padding::ZERO
            })
            .width(Fill)
            .height(Fill),
    ]
    .into()
}

fn file_listing_footer<'a>(count: usize, total: u64) -> Element<'a, Message> {
    container(
        text(file_listing_summary(count, total))
            .size(theme::metrics::UI_TEXT)
            .font(Font::MONOSPACE)
            .color(theme::palette().muted),
    )
    .height(16)
    .padding([0, FILE_CELL_PAD])
    .into()
}

/// Footer summary under a file table: `1 item 2.0 KB`, `3 items 6.0 KB`.
fn file_listing_summary(count: usize, total: u64) -> String {
    let noun = if count == 1 { "item" } else { "items" };
    format!("{count} {noun} {}", windterm_size(total))
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
    Tunnels,
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
    Exit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum KeyChord {
    FocusMode,
    FreeType,
    RemoteKeys,
    LocalNavigation,
    SplitRight,
    ToggleFiles,
    AuxiliaryShell,
    Find,
}

impl KeyChord {
    fn message(self) -> Message {
        match self {
            KeyChord::FocusMode => Message::ToggleFocusMode,
            KeyChord::FreeType => Message::ToggleFocusedFreeType,
            KeyChord::RemoteKeys => Message::ToggleFocusedRemoteKeys,
            KeyChord::LocalNavigation => Message::ToggleLocalNavigation,
            KeyChord::SplitRight => Message::Split(pane_grid::Axis::Vertical),
            KeyChord::ToggleFiles => Message::ToggleFiles,
            KeyChord::AuxiliaryShell => Message::ToggleAuxiliaryShell,
            KeyChord::Find => Message::FindOpen,
        }
    }
}

#[derive(Clone, Debug)]
enum Message {
    Tool(tools::Action),
    Search(String),
    ShowMoreProfiles,
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
    MenuOpen(menu::Menu),
    MenuHover(menu::Menu),
    MenuClose,
    MenuCommand(menu::Command),
    AskClose(usize),
    ConfirmClose(usize),
    ToggleFiles,
    ToggleAuxiliaryShell,
    FilesRefresh,
    FilesLocalUp,
    FilesRemoteUp,
    FilesShowMoreLocal,
    FilesShowMoreRemote,
    FilesOpenLocal(PathBuf),
    FilesOpenRemote(String),
    FilesLocalPathInput(String),
    FilesRemotePathInput(String),
    FilesLocalPathSubmit,
    FilesRemotePathSubmit,
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
    TogglePrivacyLock,
    ToggleFocusMode,
    ToggleLocalNavigation,
    ToggleRemoteKeys(u64),
    ToggleFocusedRemoteKeys,
    RemoteCursorPress(u64, RemoteCursorKey),
    ToggleFreeType(u64),
    ToggleFocusedFreeType,
    FreeTypeEdit(u64, text_editor::Action),
    FreeTypeSend(u64),
    FreeTypeDiscard(u64),
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
    LocalForwards(String),
    RemoteForwards(String),
    DynamicForwards(String),
    Tunnels,
    TunnelStart(String),
    TunnelStop(String),
    TunnelAcknowledge(String, bool),
    TunnelRefresh,
    TunnelAdd(String),
    TunnelEdit(String, tunnels::ForwardKind, String),
    TunnelDraftKind(tunnels::ForwardKind),
    TunnelDraftLabel(String),
    TunnelDraftBind(String),
    TunnelDraftListenPort(String),
    TunnelDraftDestinationHost(String),
    TunnelDraftDestinationPort(String),
    TunnelSaveEdit,
    TunnelDeleteEdit,
    TunnelCancelEdit,
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
    DockActivate(dock::Panel),
    DockClose(dock::Panel),
    DockMove(dock::Panel, dock::Area),
    DockToggle(dock::Panel),
    DockCollapse(dock::Area),
    DockMenu(dock::Area),
    DockPreset(bool),
    DockDragStart(dock::Area),
    DockDragMove(iced::Point),
    DockDragEnd,
    WindowResized(iced::Size),
    KeyChord(KeyChord),
    FilesSortLocal(FileSortColumn),
    FilesSortRemote(FileSortColumn),
    Exit,
    ConfirmExit,
    /// Single click on a session row. The `usize` is an index into `profiles`
    /// (the value stored in `profile_matches`), not a visible-row offset.
    SelectSession(usize),
    /// Toggle a session-tree group. Not persisted.
    ToggleSessionGroup(String),
    /// Last pointer position, used to anchor the file context menu.
    PointerMoved(iced::Point),
    /// Right-click on a Filer row. Selects the row, then opens the menu.
    OpenRemoteFileContext {
        entry: sftp::RemoteEntry,
        open_path: String,
    },
    FileContextChoose(FileContextAction),
    FileContextDismiss,
    /// Right-click on an Explorer row. Selects the row, then opens the menu.
    OpenLocalFileContext {
        path: PathBuf,
        is_dir: bool,
    },
    /// Right-click on a session row. Selects the row, then opens the menu.
    /// Does not arm the double-click timer and never opens a tab by itself.
    OpenSessionContext {
        index: usize,
    },
    SessionContextChoose(SessionContextAction),
    /// Open the Find strip for the focused pane and focus its input.
    /// Does not close an already-open strip (the menu item toggles).
    FindOpen,
    /// Edit > Find. Opens the strip, or closes it if it is already open.
    FindToggle,
    FindQuery(String),
    /// Next hit. Also the Find input's Enter submit.
    FindNext,
    FindPrev,
    FindClose,
    FindFocusProbe,
    FindFocusSynced(bool),
}

fn iced_palette_items(query: &str, library: &SnippetLibrary) -> Vec<PaletteItem> {
    command_palette::palette_items(query, library)
}

// Pane IDs are immutable within the workspace. A unique widget ID is needed
// to focus the correct inline editor even when several terminal panes are open.
fn free_type_editor_id(id: u64) -> String {
    format!("free-type-{id}")
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

/// Match only delimiters visible in the snapshot. Terminal history is never
/// searched or retained, and the pair is recalculated only when PTY content
/// generation changes. Nested pairs and reversed (closing) traversal work.
fn matched_delimiters(snapshot: &terminal_core::DisplaySnapshot) -> Option<(usize, usize)> {
    let cursor = snapshot.cells.iter().position(|cell| {
        cell.cursor && matches!(cell.character, '(' | ')' | '[' | ']' | '{' | '}')
    })?;
    let (opening, closing, forward) = match snapshot.cells[cursor].character {
        '(' => ('(', ')', true),
        ')' => ('(', ')', false),
        '[' => ('[', ']', true),
        ']' => ('[', ']', false),
        '{' => ('{', '}', true),
        '}' => ('{', '}', false),
        _ => return None,
    };
    let mut depth = 0_usize;
    if forward {
        for (index, cell) in snapshot.cells.iter().enumerate().skip(cursor) {
            let ch = cell.character;
            if ch == opening {
                depth += 1;
            } else if ch == closing {
                depth -= 1;
                if depth == 0 {
                    return Some((cursor, index));
                }
            }
        }
    } else {
        for index in (0..=cursor).rev() {
            let ch = snapshot.cells[index].character;
            if ch == closing {
                depth += 1;
            } else if ch == opening {
                depth -= 1;
                if depth == 0 {
                    return Some((index, cursor));
                }
            }
        }
    }
    None
}

/// Return cell-index spans for visible HTTP(S) URLs in a single terminal row.
/// This is a presentation hint only: it never invokes a URL handler or copies
/// remote output to the system clipboard. Only recalculated on invalidated rows.
fn visible_url_ranges(row: &[terminal_core::DisplayCell]) -> Vec<(usize, usize)> {
    let chars: Vec<char> = row.iter().map(|cell| cell.character).collect();
    let mut spans = Vec::new();
    let mut index = 0;
    while index < chars.len() {
        // Do not highlight a URL prefix embedded in a larger identifier.
        if index > 0 && (chars[index - 1].is_alphanumeric() || chars[index - 1] == '_') {
            index += 1;
            continue;
        }
        let prefix = if chars[index..].starts_with(&['h', 't', 't', 'p', 's', ':', '/', '/']) {
            8
        } else if chars[index..].starts_with(&['h', 't', 't', 'p', ':', '/', '/']) {
            7
        } else {
            index += 1;
            continue;
        };
        let host = index + prefix;
        if host >= chars.len() || !chars[host].is_ascii_alphanumeric() {
            index += 1;
            continue;
        }
        let mut end = host;
        while end < chars.len()
            && chars[end].is_ascii_graphic()
            && !matches!(chars[end], '<' | '>' | '"' | '\'' | '(' | ')' | '{' | '}')
        {
            end += 1;
        }
        // Sentence punctuation commonly follows a URL, but is not part of it.
        while end > host && matches!(chars[end - 1], '.' | ',' | ';' | ':' | '!' | '?') {
            end -= 1;
        }
        if end > host {
            spans.push((index, end));
        }
        index = end.max(index + 1);
    }
    spans
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
    highlighted_delimiters: Cell<Option<(usize, usize)>>,
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
            highlighted_delimiters: Cell::new(None),
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
    remote_key_mode: bool,
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
            iced::Event::Keyboard(keyboard::Event::KeyPressed { key, modifiers, .. })
                if self.focused && self.remote_key_mode =>
            {
                if let Some(action) = remote_vim_key(key, *modifiers) {
                    if std::env::var_os("INSPIRUM_CI_REMOTE_KEY_TRACE").is_some() {
                        eprintln!("ci_remote_key: focused canvas handled a navigation key");
                    }
                    // In modal remote-key mode the canvas owns the shortcut.
                    // Capturing it prevents the general keyboard subscription
                    // from delivering a second identical SSH cursor command.
                    return Some(
                        canvas::Action::publish(Message::RemoteCursorPress(self.id, action))
                            .and_capture(),
                    );
                }
                None
            }
            iced::Event::Keyboard(keyboard::Event::KeyPressed { .. })
                if position.is_some() && self.appearance.hide_pointer_while_typing =>
            {
                // Keyboard events belong to the PTY input subscription.
                // Capturing a Canvas redraw here drops h/j/k/l while the
                // pointer hovers over the terminal, breaking remote keys.
                // The PTY/frame bridge will repaint after input, so merely
                // update pointer state without taking ownership of the key.
                state.pointer_hidden = true;
                None
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
                // Wheel events are broadcast to every widget; only the pane
                // under the pointer may scroll (otherwise scrolling the files
                // panel also scrolls the terminal scrollback).
                let position = position?;
                let lines = wheel_scroll_lines(delta, self.cell_size().1)?;
                if self.remote_mouse_enabled(state.modifiers) {
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
        if self.focused {
            trace_terminal_click_canvas_drawn(self.id);
        }
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
            let delimiters = matched_delimiters(self.snapshot);
            state.highlighted_delimiters.set(delimiters);
            let mut new_hashes = Vec::with_capacity(self.snapshot.rows);
            for row in 0..self.snapshot.rows {
                let mut hasher = DefaultHasher::new();
                appearance_key.hash(&mut hasher);
                self.snapshot.background.hash(&mut hasher);
                self.snapshot.columns.hash(&mut hasher);
                row.hash(&mut hasher);
                if let Some(&(start, end)) = self.snapshot.row_ranges.get(row) {
                    self.snapshot.cells[start..end].hash(&mut hasher);
                    // A paired bracket on another row must invalidate its
                    // own cached geometry when the cursor moves elsewhere.
                    delimiters
                        .filter(|(a, b)| (start..end).contains(a) || (start..end).contains(b))
                        .hash(&mut hasher);
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
                let url_spans = visible_url_ranges(&self.snapshot.cells[start..end]);
                for (cell_index, cell) in self.snapshot.cells[start..end].iter().enumerate() {
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
                    // The URL accent is deliberately distinct from SGR underline.
                    // Cache::draw runs this detection only when the row changes.
                    if url_spans
                        .iter()
                        .any(|&(first, last)| (first..last).contains(&cell_index))
                    {
                        frame.fill(
                            &canvas::Path::rectangle(
                                iced::Point::new(x, y + cell_height - 4.0),
                                iced::Size::new(cell_width, 1.0),
                            ),
                            TERMINAL_ACCENT,
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
                    if state
                        .highlighted_delimiters
                        .get()
                        .is_some_and(|(a, b)| a == start + cell_index || b == start + cell_index)
                    {
                        // Thin accent below both matching delimiters. Do not
                        // repaint glyphs or change the remote terminal buffer.
                        frame.fill(
                            &canvas::Path::rectangle(
                                iced::Point::new(x, y + cell_height - 3.0),
                                iced::Size::new(cell_width, 2.0),
                            ),
                            TERMINAL_ACCENT,
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
    local_forwards: String,
    remote_forwards: String,
    dynamic_forwards: String,
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
            local_forwards: String::new(),
            remote_forwards: String::new(),
            dynamic_forwards: String::new(),
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
            local_forwards: session.ssh.local_forwards.join("; "),
            remote_forwards: session.ssh.remote_forwards.join("; "),
            dynamic_forwards: session.ssh.dynamic_forwards.join("; "),
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
                || !session.ssh.remote_command.is_empty()
                || !session.ssh.local_forwards.is_empty()
                || !session.ssh.remote_forwards.is_empty()
                || !session.ssh.dynamic_forwards.is_empty(),
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
        session.ssh.local_forwards = tunnels::parse_forward_lines(&self.local_forwards);
        session.ssh.remote_forwards = tunnels::parse_forward_lines(&self.remote_forwards);
        session.ssh.dynamic_forwards = tunnels::parse_forward_lines(&self.dynamic_forwards);

        session.ssh_args()?;
        Ok(session)
    }
}

#[derive(Clone, Debug)]
struct TunnelEditState {
    profile_key: String,
    original: Option<(tunnels::ForwardKind, String)>,
    draft: tunnels::ForwardDraft,
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
    files: FilesState,
    remote_editor: Option<RemoteEditorState>,
    /// True while the Sender dock editor owns keyboard input (cleared on pane focus).
    sender_focused: bool,
    snippets_path: PathBuf,
    snippets: SnippetLibrary,
    command_query: String,
    command_sender: String,
    command_content: text_editor::Content,
    snippet_name: String,
    snippet_body: String,
    snippet_content: text_editor::Content,
    editing_snippet: Option<String>,
    open_menu: Option<menu::Menu>,
    docks: dock::DockLayout,
    /// Dock splitter currently being dragged (WindTerm-style resize).
    dock_drag: Option<dock::Area>,
    /// Logical window size, needed to turn cursor positions into dock sizes.
    window_size: iced::Size,
    ui_state_path: Option<PathBuf>,
    dock_menu: Option<dock::Area>,
    privacy_locked: bool,
    focus_mode: bool,
    local_navigation: HashSet<u64>,
    remote_keys: HashSet<u64>,
    // Pane-local editable drafts. Content is in memory only and is never
    // forwarded to the remote PTY until an explicit Send action.
    free_type: HashMap<u64, text_editor::Content>,
    free_type_confirm: Option<u64>,
    profile_visible: usize,
    profile_matches: Vec<usize>,
    form: ConnectionForm,
    import_path: String,
    editing_profile: Option<String>,
    dialog: Option<Dialog>,
    tunnels: tunnels::Manager,
    tunnel_status: std::collections::BTreeMap<String, tunnels::Status>,
    tunnel_notice: String,
    tunnel_edit: Option<TunnelEditState>,
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
    /// Observation only. Unit panes have `terminal: None`, so a log next to
    /// `process_command` never fires. Must not change write behaviour.
    #[cfg(test)]
    pty_write_log: Vec<(u64, Vec<u8>)>,
    /// Explorer (local) column. Independent of the Filer sort.
    local_file_sort: FileSortColumn,
    local_file_sort_ascending: bool,
    /// Filer (remote) column. Independent of the Explorer sort.
    remote_file_sort: FileSortColumn,
    remote_file_sort_ascending: bool,
    /// Selected session row: index into `profiles`, not the filtered list.
    /// Cleared when the profile list or filter is rebuilt so it cannot point
    /// at a different profile.
    selected_profile: Option<usize>,
    /// `session_profile_key` captured when the row was selected.
    selected_profile_key: Option<String>,
    /// Last left-click of a session row, bound to that row's profile key.
    /// A second left-click of the same key within `SESSION_DOUBLE_CLICK`
    /// opens it, then this is cleared so one double-click gesture cannot
    /// open twice. A right-click clears it.
    session_click: Option<(Instant, String)>,
    /// Group names whose session rows are hidden. Not written to UI state.
    /// A non-empty filter still shows matching rows inside a collapsed group.
    collapsed_groups: HashSet<String>,
    /// Last pointer position. Context menus copy this into `anchor`.
    /// Updated by `Message::PointerMoved`, from the root `on_move` (press
    /// path) and from the CursorMoved subscription (captured drags).
    last_pointer: iced::Point,
    /// Open right-click menu, if any. File and session rows share this.
    context_menu: Option<ContextMenu>,
    /// Find strip is visible at the bottom of the centre workspace.
    find_open: bool,
    /// True while the Find input owns keyboard input. Cleared when the strip
    /// closes, on Escape, and when a terminal pane is focused.
    find_focused: bool,
}

const FILES_PANELS: [dock::Panel; 3] = [
    dock::Panel::Filer,
    dock::Panel::Explorer,
    dock::Panel::Transfer,
];

impl App {
    fn save_ui_state(&mut self) {
        let Some(path) = self.ui_state_path.as_deref() else {
            return;
        };
        let state = ui_state::UiState {
            theme: theme::current().as_str().to_string(),
            docks: Some(self.docks.clone()),
        };
        if let Err(error) = ui_state::save(path, &state) {
            self.status = format!("Could not save UI layout: {error}");
        }
    }

    fn boot(profiles_path: PathBuf, ssh_config: Option<PathBuf>) -> Self {
        let _slow = SlowIcedScope::start("startup");
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
        let ui_state_path = if cfg!(test) {
            None
        } else {
            Some(ui_state::path_for(&profiles_path))
        };
        let mut saved_ui = ui_state_path
            .as_deref()
            .map(ui_state::load)
            .unwrap_or_default();
        let env_theme = std::env::var("INSPIRUM_THEME")
            .ok()
            .as_deref()
            .and_then(theme::ChromeTheme::parse);
        if env_theme.is_none() && ui_state_path.is_some() {
            theme::set(
                theme::ChromeTheme::parse(&saved_ui.theme).unwrap_or(theme::ChromeTheme::Light),
            );
        }
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
            files: FilesState::new(),
            remote_editor: None,
            sender_focused: false,
            snippets_path,
            snippets,
            command_query: String::new(),
            command_sender: String::new(),
            command_content: text_editor::Content::new(),
            snippet_name: String::new(),
            snippet_body: String::new(),
            snippet_content: text_editor::Content::new(),
            editing_snippet: None,
            open_menu: None,
            dock_drag: None,
            window_size: iced::Size::new(1280.0, 800.0),
            docks: {
                let mut docks = saved_ui.docks.take().unwrap_or_default();
                docks.sanitize();
                docks.retain_available(&available_panels());
                docks.add_missing(&available_panels());
                docks
            },
            ui_state_path,
            dock_menu: None,
            privacy_locked: false,
            focus_mode: false,
            local_navigation: HashSet::new(),
            remote_keys: HashSet::new(),
            free_type: HashMap::new(),
            free_type_confirm: None,
            profile_visible: PROFILE_PAGE_SIZE,
            profile_matches,
            form: ConnectionForm::default(),
            import_path: String::new(),
            editing_profile: None,
            dialog: None,
            tunnels: tunnels::Manager::default(),
            tunnel_status: Default::default(),
            tunnel_notice: String::new(),
            tunnel_edit: None,
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
            #[cfg(test)]
            pty_write_log: Vec::new(),
            local_file_sort: FileSortColumn::Name,
            local_file_sort_ascending: true,
            remote_file_sort: FileSortColumn::Name,
            remote_file_sort_ascending: true,
            selected_profile: None,
            selected_profile_key: None,
            session_click: None,
            collapsed_groups: HashSet::new(),
            last_pointer: iced::Point::ORIGIN,
            context_menu: None,
            find_open: false,
            find_focused: false,
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
        // Indices are stable under a filter, but a rebuilt list can shift
        // them. Drop the selection rather than risk pointing at another profile.
        self.selected_profile = None;
        self.selected_profile_key = None;
        self.session_click = None;
    }

    fn new_terminal_pane(&mut self, profile: Session) -> TerminalPane {
        self.new_terminal_pane_kind(profile, false)
    }

    fn new_terminal_pane_kind(&mut self, profile: Session, sftp: bool) -> TerminalPane {
        let id = self.next_terminal_id;
        self.next_terminal_id = self.next_terminal_id.saturating_add(1);

        let mut pane = TerminalPane {
            sftp,
            auxiliary_shell: false,
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
        if self.find_focused {
            return;
        }
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
    // Never mirror input into other SSH panes if the focused source has
    // disappeared or cannot accept a write. Target IDs can be sorted before
    // the source ID, so a naive for-loop can otherwise send to the wrong
    // session before discovering that the source is unavailable.
    fn write_source_then_mirrors(
        source: u64,
        targets: &[u64],
        write: impl FnMut(u64) -> bool,
    ) -> bool {
        terminal_ux::source_first_synced_write(source, targets, write)
    }

    fn remote_terminal_mode(&self, id: u64) -> terminal_core::TerminalMode {
        self.tabs
            .iter()
            .flat_map(|tab| tab.panes.iter())
            .find(|(_, pane)| pane.id == id)
            .and_then(|(_, pane)| pane.terminal.as_ref())
            .map(|terminal| terminal.last_content().terminal_mode)
            .unwrap_or_else(terminal_core::TerminalMode::empty)
    }

    fn send_remote_cursor(&mut self, id: u64, action: RemoteCursorKey) -> bool {
        let ci_trace = std::env::var_os("INSPIRUM_CI_REMOTE_KEY_TRACE").is_some();
        if ci_trace {
            eprintln!(
                "ci_remote_key: request mode={} focus={} lock={} dialog={}",
                self.remote_keys.contains(&id),
                self.focused_pane_matches(id),
                self.privacy_locked,
                self.dialog.is_some()
            );
        }
        if self.privacy_locked
            || self.dialog.is_some()
            || self.remote_editor.is_some()
            || self.sender_focused
            || self.find_focused
            || !self.remote_keys.contains(&id)
            || !self.focused_pane_matches(id)
            || self.local_navigation.contains(&id)
            || self.free_type.contains_key(&id)
        {
            return false;
        }
        let targets = self.paste_targets(id);
        // Source first. Synchronized destinations get their *own* terminal
        // cursor mode (normal vs application), never a source-mode escape.
        Self::write_source_then_mirrors(id, &targets, |target| {
            let bytes = remote_cursor_bytes(action, self.remote_terminal_mode(target));
            let sent = self.command_terminal(target, terminal_core::BackendCommand::Write(bytes));
            if ci_trace {
                eprintln!("ci_remote_key: PTY write accepted={sent}");
            }
            sent
        })
    }

    fn send_to_terminal(&mut self, id: u64, bytes: Vec<u8>) -> bool {
        if self.find_focused {
            return false;
        }
        if !self.focused_pane_matches(id)
            || self.local_navigation.contains(&id)
            || self.free_type.contains_key(&id)
        {
            return false;
        }
        let targets = self.paste_targets(id);
        Self::write_source_then_mirrors(id, &targets, |target| {
            self.command_terminal(target, terminal_core::BackendCommand::Write(bytes.clone()))
        })
    }

    fn command_terminal(&mut self, id: u64, command: terminal_core::BackendCommand) -> bool {
        // Record the attempt before any early return. Unit tests have no PTY,
        // so this is the only observation point; it does not change behaviour.
        #[cfg(test)]
        if let terminal_core::BackendCommand::Write(bytes) = &command {
            self.pty_write_log.push((id, bytes.clone()));
        }
        // Enforce the mode at the single PTY write boundary, including
        // synchronized input, delayed clipboard callbacks and tool commands.
        if (self.local_navigation.contains(&id) || self.free_type.contains_key(&id))
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

    fn prune_free_type(&mut self) {
        let present: HashSet<u64> = self
            .tabs
            .iter()
            .flat_map(|tab| tab.panes.iter().map(|(_, pane)| pane.id))
            .collect();
        self.free_type.retain(|id, _| present.contains(id));
        self.local_navigation.retain(|id| present.contains(id));
        self.remote_keys.retain(|id| present.contains(id));
        if self
            .free_type_confirm
            .is_some_and(|id| !present.contains(&id))
        {
            self.free_type_confirm = None;
        }
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
        self.sender_focused = false;
        if index < self.tabs.len() {
            if self.active != index {
                self.close_find_strip();
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
                "Session opened.".into()
            } else {
                "Session queued.".into()
            };
        }
        self.dialog = None;
    }

    /// Open the profile at `index`, then refresh file listings when a file panel is visible.
    fn open_profile(&mut self, index: usize) -> Task<Message> {
        self.open(index);
        if self.files_open() {
            Task::batch([self.reload_files(), self.restore_find_focus()])
        } else {
            self.restore_find_focus()
        }
    }

    /// Write the selection state `SelectSession` writes.
    ///
    /// Does not arm or consume `session_click` and does not open a tab.
    /// Right-click uses this so it cannot start or finish a double-click open.
    /// The caller clears `session_click` so a right-click interrupts the gesture.
    fn select_session_row(&mut self, index: usize) -> Option<String> {
        match self.profiles.get(index).map(session_profile_key) {
            Some(key) => {
                self.selected_profile = Some(index);
                self.selected_profile_key = Some(key.clone());
                Some(key)
            }
            None => {
                self.selected_profile = None;
                self.selected_profile_key = None;
                None
            }
        }
    }

    fn files_open(&self) -> bool {
        !self.tabs.is_empty()
            && FILES_PANELS
                .iter()
                .any(|&panel| self.docks.is_visible(panel))
    }

    /// Side effects of the Filer panel becoming visible: refresh listings.
    fn files_became_visible(&mut self) -> Task<Message> {
        self.reload_files()
    }

    fn toggle_files(&mut self) {
        if self.files_open() {
            for panel in FILES_PANELS {
                self.docks.close(panel);
            }
        } else if !self.tabs.is_empty() {
            // Filer last so it ends up the active panel of its area.
            for panel in [
                dock::Panel::Transfer,
                dock::Panel::Explorer,
                dock::Panel::Filer,
            ] {
                self.docks.show(panel);
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
            self.files.remote_visible = FILES_PAGE_SIZE;
            self.files.remote_entries.clear();
            self.resort_remote_files();
            self.files.selected_remote = None;
            self.files.remote_error = None;
            self.files.remote_generation = self.files.remote_generation.wrapping_add(1);
        }
    }

    fn reload_local_files(&mut self) -> Task<Message> {
        self.files.local_path_input = self.files.local_dir.to_string_lossy().into_owned();
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
                            .map(LocalFileEntry::from_path)
                            .collect::<Vec<_>>()
                    })
                    .map_err(|error| format!("{error:#}"))
            }),
            move |result| Message::FilesLocalLoaded(generation, result),
        )
    }

    fn reload_remote_files(&mut self) -> Task<Message> {
        self.files.remote_path_input = self.files.remote_dir.clone();
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
        let files_open = self.files_open();
        let Some(job) = self.files.transfers.iter_mut().find(|job| job.id == id) else {
            return false;
        };
        let active_session = self.files.session_key.clone();
        let refresh_active = terminal
            && active_session.as_deref() == Some(job.pending.session_key.as_str())
            && files_open;

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
        let message = match message {
            Message::KeyChord(chord) => {
                if self.open_menu.is_some()
                    || self.dock_menu.is_some()
                    || self.context_menu.is_some()
                {
                    return Task::none();
                }
                // The Find input owns the keyboard. Its own chord may refresh
                // the strip; every other chord stays out of the terminal.
                if self.find_focused && !matches!(chord, KeyChord::Find) {
                    return Task::none();
                }
                return self.update(chord.message());
            }
            other => other,
        };
        if matches!(&message, Message::TogglePrivacyLock) {
            self.privacy_locked = !self.privacy_locked;
            self.ime_cursor = None;
            if self.privacy_locked {
                // The curtain replaces the tree, so the Find input is gone.
                self.close_find_strip();
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
                    | Message::ToggleAuxiliaryShell
                    | Message::FindOpen
                    | Message::FindToggle
                    | Message::ToggleLocalNavigation
                    | Message::ToggleRemoteKeys(..)
                    | Message::ToggleFocusedRemoteKeys
                    | Message::RemoteCursorPress(..)
                    | Message::ToggleFreeType(..)
                    | Message::ToggleFocusedFreeType
                    | Message::FreeTypeEdit(..)
                    | Message::FreeTypeSend(..)
                    | Message::FreeTypeDiscard(..)
                    | Message::Tool(
                        tools::Action::SendCommand
                            | tools::Action::Confirm
                            | tools::Action::TmuxAttach(..)
                            | tools::Action::TmuxCreate
                            | tools::Action::SftpTerminal
                            | tools::Action::SyncArm(..)
                            | tools::Action::SyncTarget(..)
                    )
            )
        {
            // Exit confirmation can sit on the curtain. Escape dismisses it;
            // every other locked input stays dropped.
            if matches!(self.dialog, Some(Dialog::Exit))
                && matches!(
                    &message,
                    Message::Event(iced::Event::Keyboard(keyboard::Event::KeyPressed {
                        key,
                        ..
                    })) if key.as_ref() == keyboard::Key::Named(keyboard::key::Named::Escape)
                )
            {
                return self.update(Message::CloseDialog);
            }
            return Task::none();
        }
        // An open menu owns keyboard input, including queued IME and paste
        // messages from before the overlay appeared.
        if (self.open_menu.is_some() || self.dock_menu.is_some() || self.context_menu.is_some())
            && matches!(
                &message,
                Message::TerminalImeCursor(..)
                    | Message::TerminalImeCommit(..)
                    | Message::RequestPaste
                    | Message::ClipboardRead(..)
                    | Message::ConfirmPaste
                    | Message::PanePaste(..)
                    | Message::RemoteCursorPress(..)
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
                if self.remote_keys.contains(&id) {
                    self.status = "Exit remote-key navigation before local navigation.".into();
                    return Task::none();
                }
                if self.free_type.contains_key(&id) {
                    self.status = "Finish or discard the pane's free-type draft first.".into();
                    return Task::none();
                }
                if !self.local_navigation.insert(id) {
                    self.local_navigation.remove(&id);
                    self.status = "Remote input restored for focused pane.".into();
                } else {
                    self.ime_cursor = None;
                    self.status = "Local navigation: arrows, PageUp/Down, j/k, Ctrl+u/d, g/G scroll locally. Shift+Enter or Escape exits.".into();
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
                if self.focus_mode {
                    // Focus mode does not mount the Find strip.
                    self.close_find_strip();
                }
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
                    && self.remote_editor.is_none()
                    && !self.sender_focused
                    && !self.find_focused
                    && !self.free_type.contains_key(&id)
                    && !self.remote_keys.contains(&id) =>
            {
                if let Some(bytes) = crate::keyboard::committed_text_bytes(&text) {
                    self.send_to_terminal(id, bytes);
                }
            }
            Message::TerminalImeCursor(..) | Message::TerminalImeCommit(..) => {}
            Message::TogglePrivacyLock => unreachable!("privacy lock handled before match"),
            Message::KeyChord(_) => unreachable!("key chord handled before match"),
            Message::ToggleFocusMode => unreachable!("focus mode handled before match"),
            Message::ToggleLocalNavigation => {
                unreachable!("local navigation handled before match")
            }
            Message::ToggleFocusedRemoteKeys => {
                if let Some(id) = self
                    .tabs
                    .get(self.active)
                    .and_then(|tab| tab.panes.get(tab.focus))
                    .map(|pane| pane.id)
                {
                    return self.update(Message::ToggleRemoteKeys(id));
                }
            }
            Message::ToggleRemoteKeys(id) => {
                if self.dialog.is_some()
                    || self.remote_editor.is_some()
                    || !self.focused_pane_matches(id)
                    || self.local_navigation.contains(&id)
                    || self.free_type.contains_key(&id)
                {
                    self.status =
                        "Focus a connected SSH pane with no local editor to enable remote keys."
                            .into();
                    return Task::none();
                }
                if !self.remote_keys.insert(id) {
                    self.remote_keys.remove(&id);
                    self.status = "Normal remote typing restored.".into();
                } else {
                    self.ime_cursor = None;
                    self.status = "REMOTE KEYS: h/j/k/l move the remote application; 0/$ Home/End; u/d PageUp/Down; Escape exits.".into();
                }
            }
            Message::RemoteCursorPress(id, action) => {
                if std::env::var_os("INSPIRUM_CI_REMOTE_KEY_TRACE").is_some() {
                    eprintln!("ci_remote_key: update received RemoteCursorPress");
                }
                if !self.send_remote_cursor(id, action) {
                    self.status = "Remote key cancelled: focus, mode or SSH target changed.".into();
                }
            }
            Message::ToggleFocusedFreeType => {
                if let Some(id) = self
                    .tabs
                    .get(self.active)
                    .and_then(|tab| tab.panes.get(tab.focus))
                    .map(|pane| pane.id)
                {
                    return self.update(Message::ToggleFreeType(id));
                }
            }
            Message::ToggleFreeType(id) => {
                if self.dialog.is_some()
                    || self.remote_editor.is_some()
                    || !self.focused_pane_matches(id)
                    || self.local_navigation.contains(&id)
                    || self.remote_keys.contains(&id)
                {
                    self.status = "Focus a pane in remote-input mode before composing.".into();
                    return Task::none();
                }
                if self.free_type.remove(&id).is_some() {
                    self.status = "Free-type draft discarded. Remote input restored.".into();
                } else {
                    self.free_type.insert(id, text_editor::Content::new());
                    self.ime_cursor = None;
                    self.status = "FREE TYPE: edit locally; nothing is sent until Send.".into();
                    return operation::focus(free_type_editor_id(id));
                }
            }
            Message::FreeTypeEdit(id, action) => {
                if self.dialog.is_none()
                    && self.focused_pane_matches(id)
                    && let Some(draft) = self.free_type.get_mut(&id)
                {
                    draft.perform(action);
                }
            }
            Message::FreeTypeSend(id) => {
                if self.dialog.is_some()
                    || !self.focused_pane_matches(id)
                    || self.local_navigation.contains(&id)
                {
                    return Task::none();
                }
                let Some(payload) = self.free_type.get(&id).map(|draft| draft.text()) else {
                    return Task::none();
                };
                if payload.is_empty() {
                    self.status = "The free-type draft is empty.".into();
                    return Task::none();
                }
                match terminal_ux::classify_paste(self.paste_policy, &payload) {
                    PasteDecision::Block => {
                        self.status = "Free-type send blocked by paste policy.".into();
                    }
                    PasteDecision::Confirm => {
                        self.dialog = Some(Dialog::PasteConfirm {
                            id,
                            targets: self.paste_targets(id),
                            text: payload,
                            normalize_line_endings: false,
                        });
                        self.free_type_confirm = Some(id);
                        self.status =
                            "Multiline free-type text awaits explicit confirmation.".into();
                    }
                    PasteDecision::Send => {
                        let draft = self.free_type.remove(&id).expect("free-type draft");
                        if self.send_to_terminal(id, payload.into_bytes()) {
                            self.status = "Free-type text sent to the focused terminal.".into();
                        } else {
                            self.free_type.insert(id, draft);
                            self.status = "Free-type send cancelled; draft preserved.".into();
                        }
                    }
                }
            }
            Message::FreeTypeDiscard(id) => {
                if self.dialog.is_none()
                    && self.focused_pane_matches(id)
                    && self.free_type.remove(&id).is_some()
                {
                    self.status = "Free-type draft discarded. Remote input restored.".into();
                }
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
                if self.files_open() {
                    return self.reload_files();
                }
            }
            Message::SelectNextTab => {
                if !self.tabs.is_empty() {
                    self.activate_tab((self.active + 1) % self.tabs.len());
                    if self.files_open() {
                        return self.reload_files();
                    }
                }
            }
            Message::SelectPreviousTab => {
                if !self.tabs.is_empty() {
                    self.activate_tab((self.active + self.tabs.len() - 1) % self.tabs.len());
                    if self.files_open() {
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
                            return self.restore_find_focus();
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
                self.sender_focused = self.dialog.is_none();
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
                                let task = self.update(Message::AskClose(self.active));
                                return if self.dialog.is_none() {
                                    Task::batch([task, self.restore_find_focus()])
                                } else {
                                    task
                                };
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
            Message::MenuOpen(menu) => {
                self.context_menu = None;
                self.open_menu = if self.open_menu == Some(menu) {
                    None
                } else {
                    Some(menu)
                };
            }
            Message::MenuHover(menu) => {
                if self.open_menu.is_some() {
                    self.open_menu = Some(menu);
                }
            }
            Message::MenuClose => self.open_menu = None,
            Message::MenuCommand(command) => {
                // The menu is not shown under the privacy curtain or a modal.
                // A queued Exit must not close the window or replace that modal.
                if matches!(command, menu::Command::Exit)
                    && (self.privacy_locked || self.dialog.is_some())
                {
                    return Task::none();
                }
                self.open_menu = None;
                use menu::Command as C;
                let message = match command {
                    C::NewSession => Message::New,
                    C::ImportProfiles => Message::ImportProfiles,
                    C::Tunnels => Message::Tunnels,
                    C::LockScreen => Message::TogglePrivacyLock,
                    C::Paste => Message::RequestPaste,
                    C::Find => Message::FindToggle,
                    C::Palette => Message::Commands,
                    C::ThemeLight => {
                        theme::set(theme::ChromeTheme::Light);
                        self.save_ui_state();
                        return Task::none();
                    }
                    C::ThemeDark => {
                        theme::set(theme::ChromeTheme::Dark);
                        self.save_ui_state();
                        return Task::none();
                    }
                    C::ZoomIn => Message::Scale(0.1),
                    C::ZoomOut => Message::Scale(-0.1),
                    C::FocusMode => Message::ToggleFocusMode,
                    C::ToggleSessions => Message::DockToggle(dock::Panel::Session),
                    C::ToggleFiles => Message::ToggleFiles,
                    C::LayoutBottomTabs => Message::DockPreset(false),
                    C::LayoutWindTerm => Message::DockPreset(true),
                    C::TogglePanel(panel) => Message::DockToggle(panel),
                    C::EditSession | C::Reconnect | C::CloseSession | C::Copy | C::ClosePane => {
                        let Some(tab) = self.tabs.get(self.active) else {
                            self.status = "No active session.".into();
                            return Task::none();
                        };
                        let focus = tab.focus;
                        match command {
                            C::EditSession => {
                                match self.profiles.iter().position(|p| *p == tab.profile) {
                                    Some(index) => Message::EditProfile(index),
                                    None => {
                                        self.status = "This session is not a saved profile.".into();
                                        return Task::none();
                                    }
                                }
                            }
                            C::Reconnect => Message::Reconnect(focus),
                            C::CloseSession => Message::AskClose(self.active),
                            C::Copy => match tab.panes.get(focus) {
                                Some(pane) => Message::CopySelection(pane.id),
                                None => return Task::none(),
                            },
                            _ => {
                                if tab.panes.len() < 2 {
                                    self.status = "Only one pane is open.".into();
                                    return Task::none();
                                }
                                Message::ClosePane(focus)
                            }
                        }
                    }
                    C::SyncInput => {
                        if self.tools.sync.selected_count() >= 2 {
                            Message::Tool(tools::Action::SyncArm(!self.tools.sync.armed()))
                        } else {
                            Message::Tool(tools::Action::Open(tools::Panel::Workspace))
                        }
                    }
                    C::ToggleShell => Message::ToggleAuxiliaryShell,
                    C::LocalMode => Message::ToggleLocalNavigation,
                    C::FreeType => Message::ToggleFocusedFreeType,
                    C::RemoteKeys => Message::ToggleFocusedRemoteKeys,
                    C::SplitRight => Message::Split(pane_grid::Axis::Vertical),
                    C::SplitDown => Message::Split(pane_grid::Axis::Horizontal),
                    C::NextTab => Message::SelectNextTab,
                    C::PreviousTab => Message::SelectPreviousTab,
                    C::Tool(panel) => Message::Tool(tools::Action::Open(panel)),
                    C::About => Message::About,
                    C::Exit => Message::Exit,
                };
                return self.update(message);
            }
            Message::Tunnels => {
                self.refresh_tunnel_status();
                self.dialog = Some(Dialog::Tunnels);
            }
            Message::TunnelRefresh => self.refresh_tunnel_status(),
            Message::TunnelAdd(key) => {
                if !self
                    .profiles
                    .iter()
                    .any(|profile| session_profile_key(profile) == key)
                {
                    self.tunnel_notice = "Profile no longer exists.".into();
                    return Task::none();
                }
                self.tunnel_edit = Some(TunnelEditState {
                    profile_key: key,
                    original: None,
                    draft: tunnels::ForwardDraft::default(),
                });
                self.tunnel_notice.clear();
            }
            Message::TunnelEdit(key, kind, spec) => {
                let Some(profile) = self
                    .profiles
                    .iter()
                    .find(|profile| session_profile_key(profile) == key)
                else {
                    self.tunnel_notice = "Profile no longer exists.".into();
                    return Task::none();
                };
                let Some(row) = tunnels::rows_for(profile)
                    .into_iter()
                    .find(|row| row.kind == kind && row.spec == spec)
                else {
                    self.tunnel_notice = "Tunnel entry no longer exists.".into();
                    return Task::none();
                };
                self.tunnel_edit = Some(TunnelEditState {
                    profile_key: key,
                    original: Some((kind, spec)),
                    draft: tunnels::ForwardDraft::from_row(&row),
                });
                self.tunnel_notice.clear();
            }
            Message::TunnelDraftKind(kind) => {
                if let Some(edit) = self.tunnel_edit.as_mut() {
                    edit.draft.kind = kind;
                    if kind == tunnels::ForwardKind::Dynamic {
                        edit.draft.destination_host.clear();
                        edit.draft.destination_port.clear();
                    }
                }
            }
            Message::TunnelDraftLabel(value) => {
                if let Some(edit) = self.tunnel_edit.as_mut() {
                    edit.draft.label = value;
                }
            }
            Message::TunnelDraftBind(value) => {
                if let Some(edit) = self.tunnel_edit.as_mut() {
                    edit.draft.bind_address = value;
                }
            }
            Message::TunnelDraftListenPort(value) => {
                if let Some(edit) = self.tunnel_edit.as_mut() {
                    edit.draft.listen_port = value;
                }
            }
            Message::TunnelDraftDestinationHost(value) => {
                if let Some(edit) = self.tunnel_edit.as_mut() {
                    edit.draft.destination_host = value;
                }
            }
            Message::TunnelDraftDestinationPort(value) => {
                if let Some(edit) = self.tunnel_edit.as_mut() {
                    edit.draft.destination_port = value;
                }
            }
            Message::TunnelCancelEdit => {
                self.tunnel_edit = None;
                self.tunnel_notice.clear();
            }
            Message::TunnelSaveEdit => {
                let Some(edit) = self.tunnel_edit.clone() else {
                    return Task::none();
                };
                if self.tunnels.status(&edit.profile_key).is_running() {
                    self.tunnel_notice =
                        "Stop this profile's tunnel process before changing its forwards.".into();
                    self.refresh_tunnel_status();
                    return Task::none();
                }
                let spec = match edit.draft.spec() {
                    Ok(spec) => spec,
                    Err(error) => {
                        self.tunnel_notice = format!("Cannot save tunnel: {error:#}");
                        return Task::none();
                    }
                };
                let mut profiles = self.profiles.clone();
                let Some(profile) = profiles
                    .iter_mut()
                    .find(|profile| session_profile_key(profile) == edit.profile_key)
                else {
                    self.tunnel_notice = "Profile no longer exists.".into();
                    return Task::none();
                };
                let result = if let Some((old_kind, old_spec)) = &edit.original {
                    tunnels::replace_forward(
                        profile,
                        *old_kind,
                        old_spec,
                        edit.draft.kind,
                        spec,
                        &edit.draft.label,
                    )
                } else {
                    tunnels::add_forward(profile, edit.draft.kind, spec, &edit.draft.label)
                };
                if let Err(error) =
                    result.and_then(|_| save_sessions(&self.profiles_path, &profiles))
                {
                    self.tunnel_notice = format!("Cannot save tunnel: {error:#}");
                    return Task::none();
                }
                self.profiles = profiles;
                self.refresh_profile_matches();
                self.tunnel_edit = None;
                self.tunnel_notice = "Tunnel saved.".into();
                self.refresh_tunnel_status();
            }
            Message::TunnelDeleteEdit => {
                let Some(edit) = self.tunnel_edit.clone() else {
                    return Task::none();
                };
                let Some((kind, spec)) = edit.original else {
                    self.tunnel_edit = None;
                    return Task::none();
                };
                if self.tunnels.status(&edit.profile_key).is_running() {
                    self.tunnel_notice =
                        "Stop this profile's tunnel process before deleting a forward.".into();
                    self.refresh_tunnel_status();
                    return Task::none();
                }
                let mut profiles = self.profiles.clone();
                let Some(profile) = profiles
                    .iter_mut()
                    .find(|profile| session_profile_key(profile) == edit.profile_key)
                else {
                    self.tunnel_notice = "Profile no longer exists.".into();
                    return Task::none();
                };
                if let Err(error) = tunnels::remove_forward(profile, kind, &spec)
                    .and_then(|_| save_sessions(&self.profiles_path, &profiles))
                {
                    self.tunnel_notice = format!("Cannot delete tunnel: {error:#}");
                    return Task::none();
                }
                self.profiles = profiles;
                self.refresh_profile_matches();
                self.tunnel_edit = None;
                self.tunnel_notice = "Tunnel deleted.".into();
                self.refresh_tunnel_status();
            }
            Message::TunnelAcknowledge(key, value) => self.tunnels.set_acknowledged(&key, value),
            Message::TunnelStart(key) => {
                let Some(session) = self
                    .profiles
                    .iter()
                    .find(|profile| session_profile_key(profile) == key)
                    .cloned()
                else {
                    self.tunnel_notice = "Profile no longer exists.".into();
                    return Task::none();
                };
                match self.tunnels.start(&session, self.ssh_config.as_deref()) {
                    Ok(tunnels::Status::Running { pid }) => {
                        self.tunnel_notice = format!(
                            "{} forward(s) for {} running in ssh {pid}.",
                            tunnels::rows_for(&session).len(),
                            session.name
                        );
                    }
                    Ok(tunnels::Status::Failed(error)) => {
                        self.tunnel_notice = format!("{}: {error}", session.name);
                    }
                    Ok(tunnels::Status::Stopped) => {}
                    Err(error) => self.tunnel_notice = format!("{error:#}"),
                }
                self.refresh_tunnel_status();
            }
            Message::TunnelStop(key) => {
                match self.tunnels.stop(&key) {
                    Ok(_) => {
                        self.tunnel_notice = format!("Tunnels for {key} stopped; listeners closed.")
                    }
                    Err(error) => self.tunnel_notice = format!("{error:#}"),
                }
                self.refresh_tunnel_status();
            }
            Message::CloseDialog => {
                self.free_type_confirm = None;
                self.dialog = None;
                self.editing_profile = None;
                self.form = ConnectionForm::default();
                self.files.name_input.clear();
                return self.restore_find_focus();
            }
            Message::DismissModalKey => {
                let closed_context = self.context_menu.take().is_some();
                if self.open_menu.take().is_some()
                    || self.dock_menu.take().is_some()
                    || closed_context
                {
                    return Task::none();
                }
                if self.dialog.is_some() {
                    return self.update(Message::CloseDialog);
                }
                // Captured Escape may arrive before the click-focus probe's
                // result. A focused sender or free-type editor retains its
                // Escape, but drafts in other panes do not own this key.
                let sender_owned = std::mem::take(&mut self.sender_focused);
                let active_free_type = self
                    .tabs
                    .get(self.active)
                    .and_then(|tab| tab.panes.get(tab.focus))
                    .is_some_and(|pane| self.free_type.contains_key(&pane.id));
                if !sender_owned && self.find_open && (self.find_focused || !active_free_type) {
                    self.close_find_strip();
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
                        return self.restore_find_focus();
                    }
                    if self
                        .remote_editor
                        .as_ref()
                        .is_some_and(|editor| editor.session_key == closing_key)
                    {
                        self.remote_editor = None;
                    }
                    self.tabs.remove(index);
                    self.close_find_strip();
                    self.prune_tool_panes();
                    self.prune_free_type();
                    if index < self.active {
                        self.active -= 1;
                    }
                    self.active = self.active.min(self.tabs.len().saturating_sub(1));
                    if !self.tabs.is_empty() {
                        self.refresh_workspace_displays(self.active);
                    }
                }
                if self.tabs.is_empty() {
                    self.close_find_strip();
                    if self.files_open() {
                        self.toggle_files();
                    }
                }
                self.dialog = None;
                if self.files_open() && !self.tabs.is_empty() {
                    return self.reload_files();
                }
            }
            Message::ToggleFiles => {
                if self.dialog.is_some() || self.remote_editor.is_some() {
                    return Task::none();
                }
                self.dialog = None;
                let was_open = self.files_open();
                self.toggle_files();
                self.save_ui_state();
                if !was_open && self.files_open() {
                    return self.files_became_visible();
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
            Message::FilesLocalPathInput(value) => {
                self.files.local_path_input = value;
            }
            Message::FilesRemotePathInput(value) => {
                self.files.remote_path_input = value;
            }
            Message::FilesLocalPathSubmit => {
                let Some(path) = expand_local_path(&self.files.local_path_input) else {
                    return Task::none();
                };
                if !path.is_dir() {
                    self.files.local_error =
                        Some(format!("Not a directory: {}", path.to_string_lossy()));
                    return Task::none();
                }
                return self.update(Message::FilesOpenLocal(path));
            }
            Message::FilesRemotePathSubmit => {
                let path = self.files.remote_path_input.trim().to_owned();
                if path.is_empty() {
                    return Task::none();
                }
                return self.update(Message::FilesOpenRemote(path));
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
                let files_open = self.files_open();
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
                        if files_open
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
                if self.files_open() && (!remote || session_key.as_deref() == active_session) {
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
                if self.files_open() && (!remote || session_key.as_deref() == active_session) {
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
                    self.resort_local_files();
                }
            }
            Message::FilesRemoteLoaded(generation, result) => {
                if generation == self.files.remote_generation {
                    self.files.remote_loading = false;
                    match result {
                        Ok(entries) => {
                            // Capture once per stored listing, before the sort.
                            self.files.remote_sort_reference = chrono::Local::now().date_naive();
                            self.files.remote_entries = entries;
                            self.files.remote_error = None;
                        }
                        Err(error) => {
                            self.files.remote_entries.clear();
                            self.files.remote_error = Some(error);
                        }
                    }
                    self.resort_remote_files();
                }
            }
            Message::RequestPaste => {
                if self.find_focused {
                    return Task::none();
                }
                if self
                    .tabs
                    .get(self.active)
                    .and_then(|tab| tab.panes.get(tab.focus))
                    .is_some_and(|pane| {
                        self.local_navigation.contains(&pane.id)
                            || self.free_type.contains_key(&pane.id)
                    })
                {
                    self.status = "Leave local navigation or free-type mode before pasting.".into();
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
                if self.find_focused {
                    return Task::none();
                }
                // A delayed clipboard read cannot create a paste confirmation
                // for a session that the user has already left.
                if !self.focused_pane_matches(id)
                    || targets != self.paste_targets(id)
                    || self.dialog.is_some()
                    || self.free_type.contains_key(&id)
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
                let free_type_id = self.free_type_confirm.take();
                if let Some(Dialog::PasteConfirm {
                    id,
                    targets,
                    text,
                    normalize_line_endings,
                }) = self.dialog.take()
                {
                    let prepared = normalized_paste_text(&text, normalize_line_endings);
                    // A delayed confirmation must match the original draft and
                    // the original focused/synchronized target set.
                    let draft_matches = free_type_id != Some(id)
                        || self
                            .free_type
                            .get(&id)
                            .is_some_and(|draft| draft.text() == text);
                    let eligible = draft_matches
                        && targets == self.paste_targets(id)
                        && self.focused_pane_matches(id)
                        && terminal_ux::classify_paste(self.paste_policy, &prepared)
                            != PasteDecision::Block;
                    if !eligible {
                        self.status =
                            "Paste cancelled because the draft, targets or policy changed.".into();
                    } else {
                        let draft = if free_type_id == Some(id) {
                            self.free_type.remove(&id)
                        } else {
                            None
                        };
                        if self.send_to_terminal(id, prepared.into_bytes()) {
                            self.status = "Paste sent after explicit confirmation.".into();
                        } else {
                            if let Some(draft) = draft {
                                self.free_type.insert(id, draft);
                            }
                            self.status = "Paste cancelled; local draft preserved.".into();
                        }
                    }
                }
            }
            Message::TerminalSelectStart(pane_id, id, x, y) => {
                if !self.active_pane_matches(pane_id, id) {
                    return Task::none();
                }
                trace_terminal_click_started(id);
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
                    .map(|pane| (pane.profile.clone(), pane.sftp, pane.auxiliary_shell));

                if let Some((profile, sftp, auxiliary_shell)) = profile {
                    let mut replacement = self.new_terminal_pane_kind(profile, sftp);
                    replacement.auxiliary_shell = auxiliary_shell;
                    if let Some(previous_id) = self
                        .tabs
                        .get(self.active)
                        .and_then(|tab| tab.panes.get(pane_id))
                        .map(|pane| pane.id)
                    {
                        self.free_type.remove(&previous_id);
                        self.local_navigation.remove(&previous_id);
                    }
                    let connected = replacement.terminal.is_some();
                    let failed = replacement.error.is_some();
                    if let Some(tab) = self.tabs.get_mut(self.active)
                        && let Some(pane) = tab.panes.get_mut(pane_id)
                    {
                        *pane = replacement;
                        tab.focus = pane_id;
                        self.status = if connected {
                            "Reconnected.".into()
                        } else if failed {
                            "Reconnect attempt failed; review the pane error.".into()
                        } else {
                            "Reconnect queued.".into()
                        };
                    }
                }
                self.prune_tool_panes();
            }
            Message::Split(axis) => {
                if self.dialog.is_some() || self.remote_editor.is_some() {
                    return Task::none();
                }
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
            Message::ToggleAuxiliaryShell => {
                if self.dialog.is_some() || self.remote_editor.is_some() {
                    return Task::none();
                }
                let Some(tab) = self.tabs.get(self.active) else {
                    self.status = "Open an SSH session to use the auxiliary shell.".into();
                    return Task::none();
                };
                let shell = tab
                    .panes
                    .iter()
                    .find(|(_, pane)| pane.auxiliary_shell)
                    .map(|(id, _)| *id);
                if let Some(pane) = shell {
                    if tab.panes.len() == 1 {
                        self.status =
                            "This is the last SSH pane. Close the tab to disconnect it.".into();
                        return Task::none();
                    }
                    let result = self.update(Message::ClosePane(pane));
                    self.status = "Auxiliary SSH shell disconnected; main panes preserved.".into();
                    return result;
                }
                if tab.panes.len() >= 4 {
                    self.status = "At most four SSH panes may be open per tab.".into();
                    return Task::none();
                }
                // Open an independent authenticated PTY for the same profile.
                // Never reuse the original pane's terminal backend or input.
                let profile = tab.profile.clone();
                self.tools.sync.set_armed(false);
                let mut shell = self.new_terminal_pane_kind(profile, false);
                shell.auxiliary_shell = true;
                if let Some(tab) = self.tabs.get_mut(self.active) {
                    tab.split(pane_grid::Axis::Vertical, shell);
                }
                self.status = "Independent auxiliary SSH shell opened in a right-hand pane (Ctrl/Cmd+Shift+J).".into();
            }
            Message::Focus(pane) => {
                self.sender_focused = false;
                self.find_focused = false;
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
                self.prune_free_type();
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
            Message::LocalForwards(value) => self.form.local_forwards = value,
            Message::RemoteForwards(value) => self.form.remote_forwards = value,
            Message::DynamicForwards(value) => self.form.dynamic_forwards = value,
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
                        return self.restore_find_focus();
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
                    "Ready".into()
                } else {
                    format!("Started {started} of {attempted} pending session(s).")
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
                    && self.files_open()
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

                if self.open_menu.is_some()
                    || self.dock_menu.is_some()
                    || self.context_menu.is_some()
                {
                    if key.as_ref() == keyboard::Key::Named(Named::Escape) {
                        self.open_menu = None;
                        self.dock_menu = None;
                        self.context_menu = None;
                    }
                    return Task::none();
                }
                if self.sender_focused
                    && self.dialog.is_none()
                    && key.as_ref() == keyboard::Key::Named(Named::Escape)
                {
                    self.sender_focused = false;
                    return Task::none();
                }
                // Ignored Escape (and any other key injected while the Find
                // input owns the keyboard) must not reach the PTY. Escape
                // closes the strip; every other key is dropped.
                if self.find_focused && self.dialog.is_none() {
                    if key.as_ref() == keyboard::Key::Named(Named::Escape) {
                        self.close_find_strip();
                    }
                    return Task::none();
                }
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
                    && self.remote_keys.contains(&id)
                {
                    if key.as_ref() == keyboard::Key::Named(Named::Escape) {
                        self.remote_keys.remove(&id);
                        self.status = "Normal remote typing restored.".into();
                    } else if let Some(action) = remote_vim_key(&key, modifiers) {
                        return self.update(Message::RemoteCursorPress(id, action));
                    }
                    // No arbitrary text/IME passes through modal remote keys.
                    return Task::none();
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
            Message::DockActivate(panel) => {
                let was_open = self.files_open();
                self.dock_menu = None;
                self.docks.activate(panel);
                if !self.docks.is_visible(dock::Panel::Sender) {
                    self.sender_focused = false;
                }
                self.save_ui_state();
                if !was_open && self.files_open() {
                    return self.files_became_visible();
                }
            }
            Message::DockClose(panel) => {
                self.dock_menu = None;
                self.docks.close(panel);
                if !self.docks.is_visible(dock::Panel::Sender) {
                    self.sender_focused = false;
                }
                self.save_ui_state();
            }
            Message::DockMove(panel, area) => {
                let was_open = self.files_open();
                self.dock_menu = None;
                self.docks.move_to(panel, area);
                self.docks.widen_for_file_panel(panel, area);
                if !self.docks.is_visible(dock::Panel::Sender) {
                    self.sender_focused = false;
                }
                self.save_ui_state();
                if !was_open && self.files_open() {
                    return self.files_became_visible();
                }
            }
            Message::DockToggle(panel) => {
                let was_open = self.files_open();
                self.dock_menu = None;
                self.docks.toggle(panel);
                if !self.docks.is_visible(dock::Panel::Sender) {
                    self.sender_focused = false;
                }
                self.save_ui_state();
                if !was_open && self.files_open() {
                    return self.files_became_visible();
                }
            }
            Message::DockCollapse(area) => {
                let was_open = self.files_open();
                self.dock_menu = None;
                self.docks.toggle_collapsed(area);
                if !self.docks.is_visible(dock::Panel::Sender) {
                    self.sender_focused = false;
                }
                self.save_ui_state();
                if !was_open && self.files_open() {
                    return self.files_became_visible();
                }
            }
            Message::WindowResized(size) => self.window_size = size,
            Message::DockDragStart(area) => {
                self.dock_menu = None;
                if self.dialog.is_none() {
                    self.dock_drag = Some(area);
                }
            }
            Message::DockDragMove(position) => {
                if self.dialog.is_some() {
                    self.dock_drag = None;
                    self.save_ui_state();
                } else if let Some(area) = self.dock_drag {
                    self.docks.drag_splitter(area, position, self.window_size);
                }
            }
            Message::DockDragEnd => {
                if self.dock_drag.take().is_some() {
                    self.save_ui_state();
                }
            }
            Message::DockMenu(area) => {
                self.context_menu = None;
                self.dock_menu = if self.dock_menu == Some(area) {
                    None
                } else {
                    Some(area)
                };
            }
            Message::DockPreset(windterm) => {
                let was_open = self.files_open();
                self.dock_menu = None;
                self.docks = dock::DockLayout::preset(if windterm {
                    dock::Preset::WindTermDefault
                } else {
                    dock::Preset::BottomTabs
                });
                self.docks.retain_available(&available_panels());
                self.docks.add_missing(&available_panels());
                if !self.docks.is_visible(dock::Panel::Sender) {
                    self.sender_focused = false;
                }
                self.save_ui_state();
                if !was_open && self.files_open() {
                    return self.files_became_visible();
                }
            }
            Message::FilesSortLocal(column) => {
                let (column, ascending) =
                    next_file_sort(self.local_file_sort, self.local_file_sort_ascending, column);
                self.local_file_sort = column;
                self.local_file_sort_ascending = ascending;
                self.resort_local_files();
            }
            Message::FilesSortRemote(column) => {
                let (column, ascending) = next_file_sort(
                    self.remote_file_sort,
                    self.remote_file_sort_ascending,
                    column,
                );
                self.remote_file_sort = column;
                self.remote_file_sort_ascending = ascending;
                self.resort_remote_files();
            }
            Message::Exit => {
                self.open_menu = None;
                self.dock_menu = None;
                self.context_menu = None;
                if self.tabs.is_empty() {
                    return iced::exit();
                }
                self.dialog = Some(Dialog::Exit);
            }
            Message::ConfirmExit => {
                if !matches!(self.dialog, Some(Dialog::Exit)) {
                    return Task::none();
                }
                return iced::exit();
            }
            Message::SelectSession(index) => {
                let now = Instant::now();
                let profile_key = self.profiles.get(index).map(session_profile_key);
                // The pending click is the row that was left-clicked, not
                // whatever `selected_profile` currently points at. A right-click
                // can change the selection without owning that timestamp.
                let open_now = profile_key.as_ref().is_some_and(|key| {
                    self.session_click
                        .as_ref()
                        .is_some_and(|(previous, pending)| {
                            pending == key && session_double_click(*previous, now)
                        })
                });
                // Selection state is shared with right-click. Only a left
                // click arms or consumes the double-click timer, and only
                // for the row that was left-clicked.
                if let Some(key) = self.select_session_row(index) {
                    // Consume the gesture so the opening click is not also
                    // the start of a second open.
                    self.session_click = if open_now { None } else { Some((now, key)) };
                } else {
                    self.session_click = None;
                }
                if open_now {
                    return self.open_profile(index);
                }
            }
            Message::ToggleSessionGroup(group) => {
                if !self.collapsed_groups.remove(&group) {
                    self.collapsed_groups.insert(group);
                }
            }
            Message::PointerMoved(position) => {
                self.last_pointer = position;
            }
            Message::OpenRemoteFileContext { entry, open_path } => {
                if self.privacy_locked || self.dialog.is_some() {
                    return Task::none();
                }
                // Select first so Download/Rename/Delete apply to this row.
                // Directory left-click navigates (`FilesOpenRemote`); selecting
                // here does not, so the menu's Open item is what navigates.
                let is_dir = entry.is_dir;
                let select = self.update(Message::FilesSelectRemote(entry));
                self.open_menu = None;
                self.dock_menu = None;
                self.context_menu = Some(ContextMenu::File(FileContextMenu {
                    side: FileContextSide::Remote,
                    is_dir,
                    open_path,
                    local_path: None,
                    anchor: self.last_pointer,
                }));
                return select;
            }
            Message::OpenLocalFileContext { path, is_dir } => {
                if self.privacy_locked || self.dialog.is_some() {
                    return Task::none();
                }
                // Same rule as Filer: select, do not navigate. Open does that.
                let open_path = path.to_string_lossy().into_owned();
                let local_path = path.clone();
                let select = self.update(Message::FilesSelectLocal(path));
                self.open_menu = None;
                self.dock_menu = None;
                self.context_menu = Some(ContextMenu::File(FileContextMenu {
                    side: FileContextSide::Local,
                    is_dir,
                    open_path,
                    local_path: Some(local_path),
                    anchor: self.last_pointer,
                }));
                return select;
            }
            Message::FileContextChoose(action) => {
                let Some(ContextMenu::File(menu)) = self.context_menu.take() else {
                    return Task::none();
                };
                let Some(message) = file_context_action_message(&menu, action) else {
                    return Task::none();
                };
                return self.update(message);
            }
            Message::FileContextDismiss => {
                self.context_menu = None;
            }
            Message::OpenSessionContext { index } => {
                // A right-click interrupts a pending double-click. Clear even
                // when the menu itself is refused, so the timestamp cannot be
                // reused against a selection this click just changed.
                self.session_click = None;
                if self.privacy_locked || self.dialog.is_some() {
                    return Task::none();
                }
                // Select first. A right-click must never open a session by itself.
                let Some(key) = self.select_session_row(index) else {
                    return Task::none();
                };
                self.open_menu = None;
                self.dock_menu = None;
                self.context_menu = Some(ContextMenu::Session {
                    index,
                    key,
                    anchor: self.last_pointer,
                });
            }
            Message::SessionContextChoose(action) => {
                let Some(ContextMenu::Session { index, key, .. }) = self.context_menu.take() else {
                    return Task::none();
                };
                let current = self.profiles.get(index).map(session_profile_key);
                if current.as_deref() != Some(key.as_str()) {
                    return Task::none();
                }
                match action {
                    SessionContextAction::Open => return self.open_profile(index),
                    SessionContextAction::Edit => return self.update(Message::EditProfile(index)),
                    // Same messages the Session menu dispatches.
                    SessionContextAction::NewSession => return self.update(Message::New),
                    SessionContextAction::Import => return self.update(Message::ImportProfiles),
                }
            }
            Message::FindOpen => return self.open_find_strip(),
            Message::FindToggle => {
                if self.find_open {
                    self.close_find_strip();
                    return Task::none();
                }
                return self.open_find_strip();
            }
            Message::FindQuery(query) => {
                self.find_focused = true;
                return self.tool_update(tools::Action::HistoryQuery(query));
            }
            Message::FindNext => {
                self.find_focused = true;
                return Task::batch([
                    self.tool_update(tools::Action::HistoryNext(true)),
                    operation::focus(FIND_INPUT_ID),
                ]);
            }
            Message::FindPrev => {
                self.find_focused = true;
                return Task::batch([
                    self.tool_update(tools::Action::HistoryNext(false)),
                    operation::focus(FIND_INPUT_ID),
                ]);
            }
            Message::FindClose => self.close_find_strip(),
            Message::FindFocusProbe => {
                if self.find_open && self.dialog.is_none() && !self.privacy_locked {
                    return operation::is_focused(FIND_INPUT_ID).map(Message::FindFocusSynced);
                }
            }
            Message::FindFocusSynced(focused) => {
                self.find_focused = focused && self.find_open;
                if self.find_focused {
                    self.sender_focused = false;
                }
            }
        }
        Task::none()
    }

    fn view(&self) -> Element<'_, Message> {
        if self.privacy_locked {
            // Do not render the sensitive workspace behind a translucent
            // modal: the entire view must be replaced, including tabs, host
            // labels, SFTP paths and terminal scrollback.
            let curtain: Element<'_, Message> = center(
                column![
                    text("Workspace hidden").size(30).color(fg()),
                    text("SSH connections and transfers continue while this window is hidden.")
                        .size(15)
                        .color(muted()),
                    text("This is an in-app privacy curtain, not an OS lock or encryption.")
                        .size(12)
                        .color(muted()),
                    action("Unlock workspace", Message::TogglePrivacyLock).style(primary),
                    text("Shortcut: Ctrl/Cmd + Shift + L")
                        .size(12)
                        .color(blue()),
                ]
                .spacing(18)
                .align_x(iced::Center),
            )
            .style(surface)
            .width(Fill)
            .height(Fill)
            .into();
            // Keep the workspace hidden. Only the exit confirmation may sit
            // on the curtain, and only after the OS close button asked.
            return self.cover_with_exit_confirmation(curtain);
        }
        let target = if self.dialog.is_none()
            && self.remote_editor.is_none()
            && !self.sender_focused
            && !self.find_focused
            && self.open_menu.is_none()
            && self.dock_menu.is_none()
            && self.context_menu.is_none()
        {
            self.focused_terminal_id().filter(|id| {
                !self.local_navigation.contains(id)
                    && !self.free_type.contains_key(id)
                    && !self.remote_keys.contains(id)
            })
        } else {
            None
        };
        let cursor = self
            .ime_cursor
            .filter(|(id, _)| Some(*id) == target)
            .map(|(_, cursor)| cursor);
        // Outermost wrap on every non-privacy return, including menu and
        // dialog stacks built inside `view_content`. `on_move` reports a
        // point relative to this widget (`cursor.position_in`); the root is
        // laid out at the window origin, so that matches the absolute cursor.
        // It does not capture the event and does not change layout size.
        mouse_area(terminal_ime::wrap(self.view_content(), target, cursor))
            .on_move(Message::PointerMoved)
            .into()
    }

    fn view_content(&self) -> Element<'_, Message> {
        let _slow = SlowIcedScope::start("view_layout");
        if self.focus_mode && !self.tabs.is_empty() {
            // Do not mutate the underlying dock/grid state. In focus mode the
            // workspace is reduced to the live terminal split panes, with an
            // always-visible exit action and synchronized-input warning.
            let focused: Element<'_, Message> = container(
                column![
                    row![
                        text("FOCUS MODE · Alt+Enter to exit")
                            .size(12)
                            .color(blue()),
                        text(&self.tabs[self.active].profile.name)
                            .size(12)
                            .color(muted()),
                        space::horizontal(),
                        if self.tools.sync.armed() {
                            text("SYNC INPUT ARMED").size(12).color(danger())
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
            return self.cover_with_exit_confirmation(focused);
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
        let running_tunnels = self
            .tunnel_status
            .values()
            .filter(|status| status.is_running())
            .count();

        let header = self.menu_bar();

        let mode = if self
            .focused_terminal_id()
            .is_some_and(|id| self.local_navigation.contains(&id))
        {
            "Local Mode"
        } else {
            "Remote Mode"
        };
        let status_text = |value: String| {
            text(value)
                .size(theme::metrics::STATUS_TEXT)
                .color(theme::palette().status_text)
                .wrapping(iced::widget::text::Wrapping::None)
        };
        let mut right = row![
            text(mode)
                .size(theme::metrics::STATUS_TEXT)
                .color(theme::palette().status_text)
                .wrapping(iced::widget::text::Wrapping::None)
                .font(iced::Font {
                    weight: iced::font::Weight::Bold,
                    ..iced::Font::MONOSPACE
                }),
            container(space::horizontal().width(1))
                .width(1)
                .height(12)
                .style(|_| container::Style {
                    background: Some(theme::palette().status_text.into()),
                    ..Default::default()
                }),
        ]
        .spacing(16)
        .align_y(iced::Center);
        if let Some(tab) = self.tabs.get(self.active) {
            if let Some((columns, rows)) = tab
                .panes
                .get(tab.focus)
                .and_then(|pane| pane.terminal_grid_size)
            {
                right = right.push(status_text(format!("{columns}x{rows}")));
            }
            right = right.push(status_text("ssh".to_string()));
        }
        if active_transfers > 0 {
            right = right.push(status_text(format!("{active_transfers} transfers")));
        }
        if running_tunnels > 0 {
            right = right.push(status_text(format!("{running_tunnels} tunnels")));
        }
        right = right.push(status_text(
            chrono::Local::now().format("%Y/%m/%d %H:%M").to_string(),
        ));
        let left = row![status_text(if self.status.is_empty() {
            "Ready".to_string()
        } else {
            self.status.clone()
        })]
        .spacing(16)
        .align_y(iced::Center);
        if self.tools.sync.armed() {
            let warning = container(
                text(format!(
                    "INPUT TO {} PANES",
                    self.tools.sync.selected_count()
                ))
                .size(theme::metrics::STATUS_TEXT)
                .color(Color::from_rgb8(35, 26, 8))
                .wrapping(iced::widget::text::Wrapping::None)
                .font(iced::Font {
                    weight: iced::font::Weight::Bold,
                    ..iced::Font::MONOSPACE
                }),
            )
            .padding([2, 7])
            .width(160)
            .align_x(iced::Center)
            .style(|theme| container::Style {
                background: Some(Color::from_rgb8(245, 190, 80).into()),
                ..breadcrumb_field(theme)
            });
            right = row![warning, right].spacing(12).align_y(iced::Center);
        }
        let footer = container(
            row![container(left).width(Fill).clip(true), right]
                .spacing(16)
                .align_y(iced::Center),
        )
        .padding([0, 10])
        .height(theme::metrics::STATUS_BAR_HEIGHT)
        .width(Fill)
        .align_y(iced::Center)
        .style(|_| container::Style {
            background: Some(theme::palette().status_bg.into()),
            ..Default::default()
        });

        let mut body = column![header, self.dock_body(), footer].height(Fill);

        if let Some(error) = &self.load_error {
            body = body.push(
                container(text(error).size(12).color(danger()))
                    .padding([8, 16])
                    .width(Fill)
                    .style(surface),
            );
        }

        let body: Element<'_, Message> = if let Some(open) = self.open_menu {
            stack![
                body,
                mouse_area(container(space::horizontal()).width(Fill).height(Fill))
                    .on_press(Message::MenuClose),
                container(self.menu_dropdown(open))
                    .padding(iced::Padding {
                        top: theme::metrics::MENU_BAR_HEIGHT,
                        left: open.offset(),
                        ..iced::Padding::ZERO
                    })
                    .width(Fill)
                    .height(Fill),
            ]
            .into()
        } else {
            body.into()
        };

        let body: Element<'_, Message> = match self.dock_menu {
            Some(area) => self.dock_menu_overlay(body, area),
            None => body,
        };
        let body: Element<'_, Message> = match &self.context_menu {
            Some(menu) => file_context_overlay(body, menu, self.window_size),
            None => body,
        };

        if let Some(dialog) = &self.dialog {
            let content = self.dialog_view(dialog);
            stack![
                body,
                opaque(
                    mouse_area(center(opaque(content)).style(|_| container::Style {
                        background: Some(Color::from_rgba(0.0, 0.0, 0.0, 0.35).into()),
                        ..Default::default()
                    }))
                    .on_press(Message::CloseDialog)
                )
            ]
            .into()
        } else {
            body
        }
    }

    fn menu_bar(&self) -> Element<'_, Message> {
        let mut titles = row![].align_y(iced::Center);
        for menu in menu::Menu::ALL {
            let open = self.open_menu == Some(menu);
            titles = titles.push(
                mouse_area(
                    button(
                        text(menu.title())
                            .size(theme::metrics::UI_TEXT)
                            .color(theme::palette().text)
                            .wrapping(iced::widget::text::Wrapping::None),
                    )
                    .padding([4, 8])
                    .width(menu.title_width())
                    .style(move |theme, status| menu_title(theme, status, open))
                    .on_press(Message::MenuOpen(menu)),
                )
                .on_enter(Message::MenuHover(menu)),
            );
        }
        titles = titles.push(space::horizontal()).push(
            button(
                text("Tunnels")
                    .size(theme::metrics::UI_TEXT)
                    .color(theme::palette().text)
                    .wrapping(iced::widget::text::Wrapping::None),
            )
            .padding([4, 8])
            .style(|theme, status| menu_title(theme, status, false))
            .on_press(Message::Tunnels),
        );
        container(titles)
            .height(theme::metrics::MENU_BAR_HEIGHT)
            .width(Fill)
            .align_y(iced::Center)
            .style(|_| container::Style {
                background: Some(theme::palette().chrome.into()),
                border: Border {
                    color: theme::palette().border,
                    width: 1.0,
                    radius: 0.0.into(),
                },
                ..Default::default()
            })
            .into()
    }

    fn menu_dropdown(&self, open: menu::Menu) -> Element<'_, Message> {
        let mut rows = column![].width(240);
        for entry in open.entries() {
            rows = match entry {
                menu::Entry::Separator => rows.push(
                    container(space::horizontal())
                        .width(Fill)
                        .height(1)
                        .style(|_| container::Style {
                            background: Some(theme::palette().border.into()),
                            ..Default::default()
                        }),
                ),
                menu::Entry::Item {
                    label,
                    shortcut,
                    command,
                } => rows.push(
                    button(
                        row![
                            text(label)
                                .size(theme::metrics::UI_TEXT)
                                .color(theme::palette().text),
                            space::horizontal(),
                            text(shortcut)
                                .size(theme::metrics::UI_TEXT)
                                .color(theme::palette().muted),
                        ]
                        .align_y(iced::Center),
                    )
                    .width(Fill)
                    .height(24)
                    .padding([3, 10])
                    .style(menu_item)
                    .on_press(Message::MenuCommand(command)),
                ),
            };
        }
        opaque(container(rows).padding(1).style(menu_popup_style))
    }

    fn sidebar(&self) -> Element<'_, Message> {
        let p = theme::palette();
        let filter = column![
            text_input("Filter", &self.query)
                .id("session-search")
                .on_input_maybe(self.dialog.is_none().then_some(Message::Search))
                // Enter belongs to the filter box, never to the active SSH PTY.
                .on_submit(Message::Search(self.query.clone()))
                .size(theme::metrics::UI_TEXT)
                .padding([4, 2])
                .style(session_filter),
            hline(),
        ]
        .width(Fill);
        let top = row![
            filter,
            tool_icon(icons::Icon::Plus, "New Session", Message::New),
            tool_icon(
                icons::Icon::Download,
                "Import Profiles",
                Message::ImportProfiles
            ),
        ]
        .spacing(2)
        .height(30)
        .align_y(iced::Center);

        let mut list = column![].spacing(0).width(Fill);
        let matches = self.profile_matches.len();
        let mut current_group: Option<String> = None;
        let mut count = 0;
        for (index, profile) in self
            .profile_matches
            .iter()
            .take(self.profile_visible)
            .filter_map(|&index| self.profiles.get(index).map(|profile| (index, profile)))
        {
            let group = if profile.folder.trim().is_empty() {
                "Sessions"
            } else {
                profile.folder.as_str()
            };
            let folded =
                session_group_hides_rows(self.collapsed_groups.contains(group), &self.query);
            if current_group.as_deref() != Some(group) {
                list = list.push(Self::session_group_row(group, folded));
                current_group = Some(group.to_owned());
            }
            count += 1;
            if folded {
                continue;
            }
            let open = self
                .tabs
                .iter()
                .any(|tab| session_profile_key(&tab.profile) == session_profile_key(profile));
            // `index` comes from `profile_matches` and addresses `self.profiles`.
            // A filter hides rows; it does not renumber this index.
            let selected = self.selected_profile == Some(index)
                && self.selected_profile_key.as_deref()
                    == Some(session_profile_key(profile).as_str());
            list = list.push(Self::session_tree_row(index, profile, open, selected));
        }

        if count == 0 {
            let empty = if self.query.trim().is_empty() {
                "No sessions"
            } else {
                "No matching sessions"
            };
            list = list.push(
                container(
                    text(empty)
                        .size(theme::metrics::UI_TEXT)
                        .font(iced::Font::MONOSPACE)
                        .color(p.muted),
                )
                .padding([4, 8]),
            );
        } else if count < matches {
            list = list.push(
                button(
                    text(format!("Show more sessions ({count} of {matches})"))
                        .size(theme::metrics::UI_TEXT)
                        .font(iced::Font::MONOSPACE)
                        .color(p.muted),
                )
                .width(Fill)
                .height(theme::metrics::ROW_HEIGHT)
                .padding([2, 8])
                .style(icon_button)
                .on_press(Message::ShowMoreProfiles),
            );
        }

        container(
            column![
                container(top)
                    .width(Fill)
                    .style(|_: &Theme| fill_style(theme::palette().dock)),
                scrollable(list).width(Fill).height(Fill)
            ]
            .spacing(4),
        )
        .width(Fill)
        .height(Fill)
        .padding(6)
        .style(session_panel)
        .into()
    }

    /// WindTerm session tree: group row (disclosure triangle + folder + name).
    fn session_group_row<'a>(group: &str, collapsed: bool) -> Element<'a, Message> {
        let p = theme::palette();
        let triangle = if collapsed {
            icons::Icon::TriangleRight
        } else {
            icons::Icon::TriangleDown
        };
        // 10 px triangle is 2 px wider than the old 8 px glyph. Spacing 4
        // (was 5) keeps the group name at the same x: 10+4+13+4 == 8+5+13+5.
        button(
            row![
                icons::icon(triangle, 10.0, p.text_dock),
                icons::icon(icons::Icon::Folder, 13.0, Color::from_rgb8(230, 200, 110)),
                text(group.to_owned())
                    .size(theme::metrics::UI_TEXT)
                    .font(iced::Font::MONOSPACE)
                    .color(p.text_dock),
            ]
            .spacing(4)
            .align_y(iced::Center),
        )
        .width(Fill)
        .height(theme::metrics::ROW_HEIGHT)
        .padding([0, 4])
        .style(session_row)
        .on_press(Message::ToggleSessionGroup(group.to_owned()))
        .into()
    }

    /// WindTerm session tree: flat full-width session row with a marker dot.
    fn session_tree_row<'a>(
        index: usize,
        profile: &Session,
        open: bool,
        selected: bool,
    ) -> Element<'a, Message> {
        let p = theme::palette();
        let destination = if profile.user.is_empty() {
            profile.host.clone()
        } else {
            format!("{}@{}", profile.user, profile.host)
        };
        let mut suffix = format!(" {destination}");
        if profile.favorite {
            suffix.push_str(" *");
        }
        for tag in &profile.tags {
            suffix.push_str(" #");
            suffix.push_str(tag);
        }
        // Selected fill is `selected_row`. `text_dock` on that fill is 8.9:1
        // (light) and 8.2:1 (dark); `muted` on the same fill is under 4.5:1.
        let suffix_color = if selected { p.text_dock } else { p.muted };
        let label = row![
            icons::dot(p.magenta, 7.0),
            text(profile.name.clone())
                .size(theme::metrics::UI_TEXT)
                .font(iced::Font::MONOSPACE)
                .color(p.text_dock),
            text(suffix)
                .size(theme::metrics::UI_TEXT)
                .font(iced::Font::MONOSPACE)
                .color(suffix_color)
                .wrapping(iced::widget::text::Wrapping::None),
            space::horizontal(),
        ]
        .spacing(6)
        .align_y(iced::Center);
        // `mouse_area::on_double_click` exists, but MouseArea returns early
        // when its child captures the press, and a button with `on_press`
        // captures ButtonPressed. A button without `on_press` is always
        // Status::Disabled. The second select of this same profile index
        // within SESSION_DOUBLE_CLICK opens it instead.
        let row_style = if selected {
            session_row_selected
        } else if open {
            session_row_open
        } else {
            session_row
        };
        let item = button(label)
            .width(Fill)
            .height(theme::metrics::ROW_HEIGHT)
            .padding(iced::Padding::new(0.0).left(24.0).right(4.0))
            .on_press(Message::SelectSession(index))
            .style(row_style);
        let edit_color = if selected || open {
            p.text_dock
        } else {
            p.muted
        };
        let edit = button(icons::icon(icons::Icon::Edit, 12.0, edit_color))
            .height(theme::metrics::ROW_HEIGHT)
            .padding([0, 5])
            .style(row_style)
            .on_press(Message::EditProfile(index));
        // Right presses are not captured by the row buttons, so a wrapping
        // mouse_area receives them (same pattern as file rows).
        mouse_area(row![item, edit].width(Fill).spacing(0))
            .on_right_press(Message::OpenSessionContext { index })
            .into()
    }

    fn workspace(&self) -> Element<'_, Message> {
        if self.tabs.is_empty() {
            return container(center(
                text("Double-click a session to connect, or Session > New Session (Ctrl+N)")
                    .size(theme::metrics::UI_TEXT)
                    .color(muted()),
            ))
            .width(Fill)
            .height(Fill)
            .style(list_area)
            .into();
        }

        let p = theme::palette();
        let mut tabs = row![].spacing(0);
        for (index, tab) in self.tabs.iter().enumerate() {
            let active = index == self.active;
            let focused_pane = tab.panes.get(tab.focus);
            let tab_label = focused_pane
                .and_then(|pane| pane.terminal_title.as_deref())
                .unwrap_or(&tab.profile.name)
                .to_owned();
            let connected = focused_pane.is_some_and(|pane| {
                pane.terminal.is_some() && !pane.exited && pane.error.is_none()
            });
            let label = self
                .tools
                .tab_labels
                .get(&tab.id)
                .copied()
                .unwrap_or_default();
            let color = match label {
                crate::tab_management::TabVisualLabel::None => muted(),
                crate::tab_management::TabVisualLabel::Blue => blue(),
                crate::tab_management::TabVisualLabel::Green => green(),
                crate::tab_management::TabVisualLabel::Amber => Color::from_rgb8(245, 190, 80),
                crate::tab_management::TabVisualLabel::Red => danger(),
            };
            let status: Element<'_, Message> = if connected {
                icons::dot(p.magenta, 8.0)
            } else {
                icons::icon(icons::Icon::CircleHollow, 8.0, p.muted)
            };
            let mut body = row![].spacing(4).align_y(iced::Center);
            if label != crate::tab_management::TabVisualLabel::None {
                body = body.push(
                    text(label.marker())
                        .size(theme::metrics::UI_TEXT)
                        .color(color),
                );
            }
            let body = body
                .push(status)
                .push(
                    button(text(tab_label).size(theme::metrics::UI_TEXT))
                        .padding([2, 2])
                        .style(tab_button)
                        .on_press(Message::SelectTab(index)),
                )
                .push(
                    button(icons::icon(
                        icons::Icon::Close,
                        10.0,
                        if active { p.text } else { p.muted },
                    ))
                    .padding(3)
                    .style(icon_button)
                    .on_press(Message::AskClose(index)),
                );
            let body = container(body)
                .padding(iced::Padding::new(0.0).left(10.0).right(4.0).top(2.0))
                .height(theme::metrics::STRIP_HEIGHT - 1.0)
                .align_y(iced::Center)
                .style(if active {
                    tab_active_fill
                } else {
                    tab_inactive_fill
                });
            tabs = tabs.push(if active {
                stack![
                    body,
                    container(space::horizontal())
                        .width(Fill)
                        .height(2)
                        .style(accent_fill)
                ]
                .into()
            } else {
                Element::from(body)
            });
        }
        let tab_strip = column![
            row![
                scrollable(tabs)
                    .direction(scrollable::Direction::Horizontal(
                        scrollable::Scrollbar::new().width(2).scroller_width(2)
                    ))
                    .width(Fill),
                button(icons::icon(icons::Icon::Plus, 12.0, p.text))
                    .padding(6)
                    .style(icon_button)
                    .on_press(Message::New),
            ]
            .height(theme::metrics::STRIP_HEIGHT - 1.0)
            .align_y(iced::Center),
            hline(),
        ];

        let dock = self.terminals();

        // The strip is omitted when closed so the terminal's computed rows
        // stay exactly the closed-workspace layout.
        let mut centre = column![
            container(tab_strip).style(surface),
            container(dock).height(Fill).style(border_fill),
        ];
        if self.find_open {
            centre = centre.push(self.find_strip());
        }

        container(centre).height(Fill).width(Fill).into()
    }

    /// Mode/state labels and reconnect/close controls for split-pane headers.
    fn pane_header_parts<'a>(
        &'a self,
        id: pane_grid::Pane,
        pane: &'a TerminalPane,
    ) -> (
        iced::widget::Row<'a, Message>,
        iced::widget::Row<'a, Message>,
    ) {
        let state = if pane.exited {
            Some(("EXITED", muted()))
        } else if pane.error.is_some() {
            Some(("ERROR", danger()))
        } else if pane.terminal.is_some() {
            None
        } else {
            Some(("CONNECTING", blue()))
        };
        let mode = if self.free_type.contains_key(&pane.id) {
            Some("FREE TYPE")
        } else if self.local_navigation.contains(&pane.id) {
            Some("LOCAL NAV")
        } else if self.remote_keys.contains(&pane.id) {
            Some("REMOTE KEYS")
        } else if pane.auxiliary_shell {
            Some("AUX SHELL")
        } else {
            None
        };
        let mut labels = row![].spacing(6).align_y(iced::Center);
        if let Some(mode) = mode {
            labels = labels.push(
                text(mode)
                    .size(10)
                    .color(blue())
                    .wrapping(iced::widget::text::Wrapping::None),
            );
        }
        if let Some((label, color)) = state {
            labels = labels.push(
                text(label)
                    .size(10)
                    .color(color)
                    .wrapping(iced::widget::text::Wrapping::None),
            );
        }
        let mut controls = row![].spacing(0).align_y(iced::Center);
        if pane.exited || pane.error.is_some() {
            controls = controls.push(compact_action("Reconnect", Message::Reconnect(id)));
        }
        controls = controls.push(compact_action("x", Message::ClosePane(id)));
        (labels, controls)
    }

    fn terminals(&self) -> Element<'_, Message> {
        let tab = &self.tabs[self.active];
        let multi_pane = tab.panes.len() > 1;
        let grid = pane_grid(&tab.panes, |id, pane, _| {
            let focused = tab.focus == id;
            let live = !pane.exited && pane.error.is_none() && pane.terminal.is_some();
            let header = responsive(move |_size| {
                let p = theme::palette();
                let status: Element<'_, Message> = if live {
                    icons::dot(p.magenta, 8.0)
                } else {
                    icons::icon(icons::Icon::CircleHollow, 8.0, p.muted)
                };
                let (labels, controls) = self.pane_header_parts(id, pane);
                let name = container(
                    text(
                        pane.terminal_title
                            .as_deref()
                            .unwrap_or(&pane.profile.name)
                            .to_owned(),
                    )
                    .size(12)
                    .color(p.text)
                    .wrapping(iced::widget::text::Wrapping::None),
                )
                .width(Fill)
                .clip(true);
                row![status, name, labels, controls]
                    .spacing(6)
                    .height(20)
                    .align_y(iced::Center)
                    .into()
            });
            let title = pane_grid::TitleBar::new(column![
                container(space::horizontal())
                    .width(Fill)
                    .height(2)
                    .style(if focused { accent_fill } else { pane_header_fill }),
                container(header).padding([0, 8]).height(20).align_y(iced::Center),
            ])
            .style(pane_header_fill);

            let terminal_body: Element<'_, Message> = if let Some(error) = &pane.error {
                column![
                    text("Unable to start SSH session").size(16).color(danger()),
                    text(error).size(13).color(muted()),
                    space::vertical().height(8),
                    text("The saved profile was not altered.")
                        .size(12)
                        .color(muted()),
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
                        && self.remote_editor.is_none()
                        && !self.sender_focused
                        && !self.find_focused,
                    remote_key_mode: self.remote_keys.contains(&pane.id),
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
                    .color(fg())
                    .width(Fill)
                    .height(Fill)
                    .into()
            };

            // Keep the resize sensor attached ONLY to the terminal viewport,
            // not the separate local draft editor below it.
            let terminal_view: Element<'_, Message> = sensor(
                container(terminal_body)
                    .padding(if pane.display.is_some() { 0 } else { 14 })
                    .width(Fill)
                    .height(Fill),
            )
            .key(pane.id)
            .on_resize(move |size| Message::TerminalResized(pane.id, size))
            .into();
            let pane_terminal_id = pane.id;
            let body: Element<'_, Message> = if let Some(draft) = self.free_type.get(&pane.id) {
                column![
                    terminal_view,
                    container(
                        column![
                            text("FREE TYPE · local editable draft · remote input suspended · Ctrl/Cmd+Shift+E")
                                .size(11)
                                .color(blue()),
                            text_editor(draft)
                                .id(free_type_editor_id(pane_terminal_id))
                                .placeholder("Compose here; Send is always explicit")
                                .on_action(move |action| Message::FreeTypeEdit(pane_terminal_id, action))
                                .height(96)
                                .padding(8),
                            row![
                                action("Send explicitly", Message::FreeTypeSend(pane.id))
                                    .style(primary),
                                action("Discard", Message::FreeTypeDiscard(pane.id)),
                                text("Multiline still requires paste confirmation.")
                                    .size(11)
                                    .color(muted()),
                            ]
                            .spacing(8)
                            .align_y(iced::Center),
                        ]
                        .spacing(5),
                    )
                    .padding(8)
                    .style(surface),
                ]
                .spacing(4)
                .height(Fill)
                .into()
            } else if self.remote_keys.contains(&pane.id) {
                column![
                    terminal_view,
                    text("REMOTE KEYS · real SSH key input · h/j/k/l · 0/$ · u/d · Esc exits")
                        .size(11)
                        .color(blue()),
                    row![
                        action("Left", Message::RemoteCursorPress(pane.id, RemoteCursorKey::Left)),
                        action("Up", Message::RemoteCursorPress(pane.id, RemoteCursorKey::Up)),
                        action("Down", Message::RemoteCursorPress(pane.id, RemoteCursorKey::Down)),
                        action("Right", Message::RemoteCursorPress(pane.id, RemoteCursorKey::Right)),
                        action("Exit", Message::ToggleRemoteKeys(pane.id)),
                    ]
                    .spacing(5),
                    row![
                        action("Home", Message::RemoteCursorPress(pane.id, RemoteCursorKey::Home)),
                        action("End", Message::RemoteCursorPress(pane.id, RemoteCursorKey::End)),
                        action("PgUp", Message::RemoteCursorPress(pane.id, RemoteCursorKey::PageUp)),
                        action("PgDn", Message::RemoteCursorPress(pane.id, RemoteCursorKey::PageDown)),
                    ]
                    .spacing(5),
                ]
                .spacing(4)
                .height(Fill)
                .into()
            } else {
                terminal_view
            };
            // Fill the pane edge to edge with the terminal's own background so
            // no chrome colour shows around the canvas or while resizing.
            let terminal_bg = pane.display.as_ref().map(|snapshot| {
                let [r, g, b] = snapshot.background;
                Color::from_rgb8(r, g, b)
            });
            let content = pane_grid::Content::new(body).style(move |theme: &Theme| match terminal_bg {
                Some(color) => fill_style(color),
                None => pane_body(theme),
            });
            if multi_pane {
                content.title_bar(title)
            } else {
                content
            }
        })
        .spacing(1)
        .on_click(Message::Focus)
        .on_drag(Message::Drag)
        .on_resize(8, Message::Resize)
        .height(Fill);
        container(grid).height(Fill).style(border_fill).into()
    }

    /// Rebuild Explorer display order from the current listing and sort.
    /// Local rows ignore the stored remote reference date. Not called from
    /// view construction.
    fn resort_local_files(&mut self) {
        self.files.local_order = file_display_order(
            &self.files.local_entries,
            self.local_file_sort,
            self.local_file_sort_ascending,
            self.files.remote_sort_reference,
        );
        self.files.local_sort_runs += 1;
    }

    /// Rebuild Filer display order from the current listing, sort, and the
    /// reference date captured when that listing was stored.
    fn resort_remote_files(&mut self) {
        self.files.remote_order = file_display_order(
            &self.files.remote_entries,
            self.remote_file_sort,
            self.remote_file_sort_ascending,
            self.files.remote_sort_reference,
        );
        self.files.remote_sort_runs += 1;
    }

    /// Local and remote file tables laid out `table_width` wide.
    ///
    /// Walks the cached display order and takes the visible page. Does not sort.
    fn file_lists(&self, table_width: f32) -> (Column<'_, Message>, Column<'_, Message>) {
        let mut local_list = column![file_listing_header(
            table_width,
            false,
            self.local_file_sort,
            self.local_file_sort_ascending,
        )];
        if self.files.local_loading {
            local_list =
                local_list.push(text("Loading local directory...").size(12).color(muted()));
        } else if let Some(error) = &self.files.local_error {
            local_list = local_list.push(text(error).size(12).color(danger()));
        } else if self.files.local_entries.is_empty() {
            local_list = local_list.push(text("Directory is empty.").size(12).color(muted()));
        } else {
            let order = &self.files.local_order;
            let entries = &self.files.local_entries;
            let visible = self.files.local_visible;
            for entry in order
                .iter()
                .filter_map(|&index| entries.get(index))
                .take(visible)
            {
                let cells = FileListingCells {
                    name: display_leaf(&entry.name),
                    is_dir: entry.is_dir,
                    modified: format_local_time(entry.modified),
                    kind: windterm_type(&entry.name, entry.is_dir),
                    size: entry
                        .size
                        .filter(|_| !entry.is_dir)
                        .map(windterm_size)
                        .unwrap_or_default(),
                    permissions: entry.permissions.clone(),
                    owner: entry.owner.clone(),
                    group: entry.group.clone(),
                };
                let selected = self.files.selected_local.as_ref() == Some(&entry.path);
                let activate = if entry.is_dir {
                    Message::FilesOpenLocal(entry.path.clone())
                } else {
                    Message::FilesSelectLocal(entry.path.clone())
                };
                local_list = local_list.push(file_listing_row(
                    cells,
                    activate,
                    selected,
                    table_width,
                    Some(Message::OpenLocalFileContext {
                        path: entry.path.clone(),
                        is_dir: entry.is_dir,
                    }),
                ));
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

        let mut remote_list = column![file_listing_header(
            table_width,
            true,
            self.remote_file_sort,
            self.remote_file_sort_ascending,
        )];
        if self.files.remote_loading {
            remote_list =
                remote_list.push(text("Loading remote directory...").size(12).color(muted()));
        } else if let Some(error) = &self.files.remote_error {
            remote_list = remote_list.push(text(error).size(12).color(danger()));
        } else if self.files.remote_entries.is_empty() {
            remote_list = remote_list.push(text("Directory is empty.").size(12).color(muted()));
        } else {
            let order = &self.files.remote_order;
            let entries = &self.files.remote_entries;
            let visible = self.files.remote_visible;
            for entry in order
                .iter()
                .filter_map(|&index| entries.get(index))
                .take(visible)
            {
                let cells = FileListingCells {
                    name: display_leaf(&entry.name),
                    is_dir: entry.is_dir,
                    modified: self.files.remote_modified_label(entry),
                    kind: windterm_type(&entry.name, entry.is_dir),
                    size: entry
                        .size
                        .filter(|_| !entry.is_dir)
                        .map(windterm_size)
                        .unwrap_or_default(),
                    permissions: entry.permissions.clone().unwrap_or_default(),
                    owner: entry.owner.clone().unwrap_or_default(),
                    group: entry.group.clone().unwrap_or_default(),
                };
                let open_path = sftp::join_remote(&self.files.remote_dir, &entry.name);
                let selected = self
                    .files
                    .selected_remote
                    .as_ref()
                    .is_some_and(|selected| selected.name == entry.name);
                let activate = if entry.is_dir {
                    Message::FilesOpenRemote(open_path.clone())
                } else {
                    Message::FilesSelectRemote(entry.clone())
                };
                remote_list = remote_list.push(file_listing_row(
                    cells,
                    activate,
                    selected,
                    table_width,
                    Some(Message::OpenRemoteFileContext {
                        entry: entry.clone(),
                        open_path,
                    }),
                ));
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

        if !self.files.local_loading && self.files.local_error.is_none() {
            let total = self
                .files
                .local_entries
                .iter()
                .filter(|entry| !entry.is_dir)
                .filter_map(|entry| entry.size)
                .sum();
            local_list =
                local_list.push(file_listing_footer(self.files.local_entries.len(), total));
        }
        if !self.files.remote_loading && self.files.remote_error.is_none() {
            let total = self
                .files
                .remote_entries
                .iter()
                .filter(|entry| !entry.is_dir)
                .filter_map(|entry| entry.size)
                .sum();
            remote_list =
                remote_list.push(file_listing_footer(self.files.remote_entries.len(), total));
        }

        (local_list, remote_list)
    }

    fn file_listing_panels(&self) -> (Element<'_, Message>, Element<'_, Message>) {
        let local_panel = column![
            files_path_bar(
                text_input("Local directory path", &self.files.local_path_input)
                    .size(12)
                    .style(files_path_input)
                    .on_input(Message::FilesLocalPathInput)
                    .on_submit(Message::FilesLocalPathSubmit)
                    .width(Fill)
                    .into(),
                action("Go", Message::FilesLocalPathSubmit),
                Message::FilesLocalUp,
            ),
            responsive(move |size| {
                container(
                    scrollable(
                        self.file_lists(size.width.max(file_listing_table_width()))
                            .0,
                    )
                    .direction(scrollable::Direction::Both {
                        vertical: scrollable::Scrollbar::default(),
                        horizontal: scrollable::Scrollbar::default(),
                    })
                    .height(Fill),
                )
                .width(Fill)
                .height(Fill)
                .style(list_area)
                .into()
            }),
        ]
        .width(Fill)
        .height(Fill);

        let remote_panel = column![
            files_path_bar(
                text_input("Remote directory path", &self.files.remote_path_input)
                    .size(12)
                    .style(files_path_input)
                    .on_input(Message::FilesRemotePathInput)
                    .on_submit(Message::FilesRemotePathSubmit)
                    .width(Fill)
                    .into(),
                action("Go", Message::FilesRemotePathSubmit),
                Message::FilesRemoteUp,
            ),
            responsive(move |size| {
                container(
                    scrollable(
                        self.file_lists(size.width.max(file_listing_table_width()))
                            .1,
                    )
                    .direction(scrollable::Direction::Both {
                        vertical: scrollable::Scrollbar::default(),
                        horizontal: scrollable::Scrollbar::default(),
                    })
                    .height(Fill),
                )
                .width(Fill)
                .height(Fill)
                .style(list_area)
                .into()
            }),
        ]
        .width(Fill)
        .height(Fill);
        (local_panel.into(), remote_panel.into())
    }

    fn files_unavailable(&self) -> Element<'_, Message> {
        container(
            text("Open a session to browse files")
                .size(13)
                .color(muted()),
        )
        .center(Fill)
        .into()
    }

    /// Filer dock panel (spec 5.2): remote path bar, table, actions, editor.
    fn filer_panel(&self) -> Element<'_, Message> {
        if self.tabs.is_empty() {
            return self.files_unavailable();
        }
        let (_, remote_panel) = self.file_listing_panels();
        let actions = files_action_row(vec![
            action("<- Download", Message::FilesRequestDownload).into(),
            action("New remote folder", Message::FilesRequestMkdirRemote).into(),
            action("Rename remote", Message::FilesRequestRenameRemote).into(),
            action("Edit remote", Message::FilesRequestEditRemote).into(),
            action("Delete remote", Message::FilesRequestDeleteRemote).into(),
        ]);
        column![remote_panel, actions, self.remote_editor_section()]
            .height(Fill)
            .width(Fill)
            .into()
    }

    /// Explorer dock panel: local path bar, table and local actions.
    fn explorer_panel(&self) -> Element<'_, Message> {
        if self.tabs.is_empty() {
            return self.files_unavailable();
        }
        let (local_panel, _) = self.file_listing_panels();
        let actions = files_action_row(vec![
            action("Upload ->", Message::FilesRequestUpload).into(),
            action("New local folder", Message::FilesRequestMkdirLocal).into(),
            action("Rename local", Message::FilesRequestRenameLocal).into(),
            action("Delete local", Message::FilesRequestDeleteLocal).into(),
        ]);
        column![local_panel, actions]
            .height(Fill)
            .width(Fill)
            .into()
    }

    /// Transfer dock panel (spec 5.3): one block per job with a progress bar.
    fn transfer_panel(&self) -> Element<'_, Message> {
        if self.tabs.is_empty() {
            return self.files_unavailable();
        }
        let active_session = self.files.session_key.as_deref();
        let mut transfer_list = column![].spacing(8).padding(8);
        let mut visible_transfers = 0usize;
        for job in self
            .files
            .transfers
            .iter()
            .rev()
            .filter(|job| active_session == Some(job.pending.session_key.as_str()))
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

            let (name, verb, direction, destination) = transfer_job_summary(job);
            let fraction = match job.total {
                Some(total) if total > 0 => {
                    (job.transferred as f64 / total as f64).clamp(0.0, 1.0) as f32
                }
                _ if job.state == TransferJobState::Completed => 1.0,
                _ => 0.0,
            };
            let detail_line = if job.error.is_empty() {
                progress
            } else {
                format!(": {}", job.error)
            };
            let state_color = match job.state {
                TransferJobState::Completed => green(),
                TransferJobState::Failed => danger(),
                TransferJobState::Running | TransferJobState::Queued => blue(),
                TransferJobState::Cancelled => theme::palette().text_dock,
            };
            transfer_list = transfer_list.push(
                column![
                    row![
                        icons::icon(icons::Icon::File, 12.0, theme::palette().text),
                        text(format!(
                            "{name} - {} {verb}",
                            format_file_size(Some(job.transferred))
                        ))
                        .size(12),
                        space::horizontal(),
                        text(format!("{destination} {direction}"))
                            .size(theme::metrics::UI_TEXT)
                            .color(theme::palette().text_dock),
                        text(state).size(theme::metrics::UI_TEXT).color(state_color),
                        text(detail_line)
                            .size(theme::metrics::UI_TEXT)
                            .color(theme::palette().text_dock),
                        controls,
                    ]
                    .spacing(6)
                    .align_y(iced::Center),
                    progress_bar(0.0..=1.0, fraction)
                        .length(Fill)
                        .girth(13)
                        .style(transfer_progress),
                ]
                .spacing(3),
            );
        }
        if visible_transfers == 0 {
            transfer_list = transfer_list.push(
                text("No transfers for this session.")
                    .size(11)
                    .color(muted()),
            );
        }

        container(scrollable(transfer_list).height(Fill).width(Fill))
            .width(Fill)
            .height(Fill)
            .style(list_area)
            .into()
    }

    fn remote_editor_section(&self) -> Element<'_, Message> {
        let active_session = self.files.session_key.as_deref();
        let section = if let Some(editor) = self
            .remote_editor
            .as_ref()
            .filter(|editor| active_session == Some(editor.session_key.as_str()))
        {
            // Rendering must not wait on an in-flight SFTP save.
            let dirty = editor.dirty;
            container(
                        column![
                            row![
                                text(format!("EDIT · {}", editor.remote)).size(12).color(blue()),
                                if dirty {
                                    text("MODIFIED").size(10).color(danger())
                                } else {
                                    text("SAVED").size(10).color(green())
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
                                            .color(danger()),
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
                                            .color(danger()),
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
                                container(text(error).size(11).color(danger()))
                                    .padding(8)
                                    .style(card)
                            } else {
                                container(
                                    text("Saving re-checks the remote contents and uses staged replacement; closing never uploads automatically.")
                                        .size(11)
                                        .color(muted())
                                )
                                .padding(4)
                            },
                        ]
                        .spacing(6)
                    )
                    .padding(8)
                    .style(active_card)
        } else {
            container(column![]).height(0).padding(0)
        };
        section.into()
    }

    fn refresh_tunnel_status(&mut self) {
        self.tunnel_status.clear();
        for profile in &self.profiles {
            let key = session_profile_key(profile);
            let status = self.tunnels.status(&key);
            self.tunnel_status.insert(key, status);
        }
    }

    fn tunnels_view(&self) -> Element<'_, Message> {
        let mut tunnel_list = column![text("Saved forwards").size(11).color(muted()),].spacing(8);

        if self
            .profiles
            .iter()
            .all(|profile| tunnels::rows_for(profile).is_empty())
        {
            tunnel_list = tunnel_list.push(
                text("No tunnels yet. Use + on a profile to create one.")
                    .size(12)
                    .color(muted()),
            );
        }

        for profile in &self.profiles {
            let key = session_profile_key(profile);
            let status = self
                .tunnel_status
                .get(&key)
                .cloned()
                .unwrap_or(tunnels::Status::Stopped);
            let status_color = if status.is_running() {
                green()
            } else {
                muted()
            };
            let mut group = column![
                row![
                    text(truncate(&profile.name, 22)).size(13),
                    space::horizontal(),
                    text(status.label()).size(10).color(status_color),
                    action("+", Message::TunnelAdd(key.clone())),
                ]
                .spacing(6)
                .align_y(iced::Center)
            ]
            .spacing(4);

            for row_data in tunnels::rows_for(profile) {
                let selected = self.tunnel_edit.as_ref().is_some_and(|edit| {
                    edit.profile_key == key
                        && edit.original.as_ref().is_some_and(|(kind, spec)| {
                            *kind == row_data.kind && spec == &row_data.spec
                        })
                });
                let subtitle = if row_data.kind == tunnels::ForwardKind::Dynamic {
                    format!("{} · {}", row_data.kind.label(), row_data.listen)
                } else {
                    format!(
                        "{} · {}  →  {}",
                        row_data.kind.label(),
                        row_data.listen,
                        row_data.target
                    )
                };
                let item = button(
                    column![
                        text(truncate(&row_data.label, 28)).size(13),
                        text(truncate(&subtitle, 42)).size(10).color(muted()),
                    ]
                    .spacing(2),
                )
                .on_press(Message::TunnelEdit(
                    key.clone(),
                    row_data.kind,
                    row_data.spec.clone(),
                ))
                .width(Fill)
                .padding(8);
                group = group.push(if selected {
                    item.style(primary)
                } else {
                    item.style(quiet)
                });
            }

            let acknowledged = self.tunnels.acknowledged(&key);
            if session_requires_forward_risk_ack(profile) && !status.is_running() {
                let ack_key = key.clone();
                group = group.push(
                    iced::widget::checkbox(acknowledged)
                        .label("Allow non-loopback bind")
                        .on_toggle(move |value| Message::TunnelAcknowledge(ack_key.clone(), value)),
                );
            }
            group = group.push(if status.is_running() {
                action("Stop", Message::TunnelStop(key.clone()))
            } else {
                action("Start", Message::TunnelStart(key.clone()))
            });
            tunnel_list = tunnel_list.push(container(group).padding(8).style(card));
        }

        let editor: Element<'_, Message> = if let Some(edit) = &self.tunnel_edit {
            let title = if edit.original.is_some() {
                "Edit Tunnel"
            } else {
                "New Tunnel"
            };
            let kind_row = row![
                action(
                    "Local",
                    Message::TunnelDraftKind(tunnels::ForwardKind::Local)
                )
                .style(if edit.draft.kind == tunnels::ForwardKind::Local {
                    primary
                } else {
                    quiet
                }),
                action(
                    "Remote",
                    Message::TunnelDraftKind(tunnels::ForwardKind::Remote)
                )
                .style(if edit.draft.kind == tunnels::ForwardKind::Remote {
                    primary
                } else {
                    quiet
                }),
                action(
                    "Dynamic",
                    Message::TunnelDraftKind(tunnels::ForwardKind::Dynamic)
                )
                .style(if edit.draft.kind == tunnels::ForwardKind::Dynamic {
                    primary
                } else {
                    quiet
                }),
            ]
            .spacing(8);

            let destination = if edit.draft.kind == tunnels::ForwardKind::Dynamic {
                column![
                    text("Dynamic SOCKS tunnel").size(12).color(muted()),
                    text("SOCKS clients may send hostnames through the tunnel for remote resolution; there is no fixed destination host for -D.")
                        .size(11)
                        .color(muted()),
                ]
                .spacing(5)
            } else {
                column![
                    text("Destination").size(13),
                    row![
                        column![
                            text("Host").size(11).color(fg()),
                            text_input("10.1.10.10", &edit.draft.destination_host)
                                .style(flat_input)
                                .on_input(Message::TunnelDraftDestinationHost)
                                .size(13)
                                .padding([3, 6])
                        ]
                        .spacing(3)
                        .width(Fill),
                        column![
                            text("Port").size(11).color(fg()),
                            text_input("8000", &edit.draft.destination_port)
                                .style(flat_input)
                                .on_input(Message::TunnelDraftDestinationPort)
                                .size(13)
                                .padding([3, 6])
                        ]
                        .spacing(3)
                        .width(120),
                    ]
                    .spacing(8)
                ]
                .spacing(5)
            };

            container(
                column![
                    text(title).size(13),
                    text("WindTerm-style structured tunnel editor")
                        .size(11)
                        .color(muted()),
                    text("Type").size(11).color(fg()),
                    kind_row,
                    column![
                        text("Label").size(11).color(fg()),
                        text_input("e.g. database", &edit.draft.label)
                            .style(flat_input)
                            .on_input(Message::TunnelDraftLabel)
                            .size(13)
                            .padding([3, 6])
                    ]
                    .spacing(3),
                    text("Listen").size(13),
                    row![
                        column![
                            text("Bind address").size(11).color(fg()),
                            text_input("127.0.0.1", &edit.draft.bind_address)
                                .style(flat_input)
                                .on_input(Message::TunnelDraftBind)
                                .size(13)
                                .padding([3, 6])
                        ]
                        .spacing(3)
                        .width(Fill),
                        column![
                            text("Port").size(11).color(fg()),
                            text_input("2000", &edit.draft.listen_port)
                                .style(flat_input)
                                .on_input(Message::TunnelDraftListenPort)
                                .size(13)
                                .padding([3, 6])
                        ]
                        .spacing(3)
                        .width(120),
                    ]
                    .spacing(8),
                    iced::widget::checkbox(true)
                        .label("Check requested listener is available before reporting Running"),
                    destination,
                    if !self.tunnel_notice.is_empty() {
                        text(&self.tunnel_notice).size(12).color(danger())
                    } else {
                        text("").size(1)
                    },
                    row![
                        if edit.original.is_some() {
                            action("Delete", Message::TunnelDeleteEdit).style(button::danger)
                        } else {
                            action("Reset", Message::TunnelCancelEdit)
                        },
                        space::horizontal(),
                        action("Cancel", Message::TunnelCancelEdit),
                        action("Save", Message::TunnelSaveEdit).style(primary),
                    ]
                    .spacing(8)
                    .align_y(iced::Center),
                ]
                .spacing(8),
            )
            .padding(8)
            .width(Fill)
            .style(card)
            .into()
        } else {
            container(
                column![
                    text("Tunnel").size(13),
                    text("Choose a tunnel on the left, or press + beside a profile to add one.")
                        .size(13)
                        .color(muted()),
                    text("The editor exposes Local, Remote and Dynamic forwards as structured fields instead of raw OpenSSH -L/-R/-D strings.")
                        .size(11)
                        .color(muted()),
                ]
                .spacing(8),
            )
            .padding(8)
            .width(Fill)
            .style(card)
            .into()
        };

        let body = column![
            row![
                text("Tunnel Manager").size(13),
                space::horizontal(),
                action("Refresh", Message::TunnelRefresh),
                action("Close", Message::CloseDialog).style(primary),
            ]
            .spacing(8)
            .align_y(iced::Center),
            text("Manage saved SSH port forwards with a WindTerm-style list-and-editor workflow. Running tunnel processes are still isolated per profile and use OpenSSH ExitOnForwardFailure.")
                .size(12)
                .color(muted()),
            row![
                container(scrollable(tunnel_list).height(430))
                    .padding(8)
                    .width(300)
                    .style(card),
                editor,
            ]
            .spacing(8)
            .height(Fill),
        ]
        .spacing(8);

        container(body).padding(6).width(Fill).height(Fill).into()
    }

    fn dialog_view<'a>(&'a self, dialog: &'a Dialog) -> Element<'a, Message> {
        let title: &'static str = match dialog {
            Dialog::Tools => "Tools",
            Dialog::Connection => {
                if self.editing_profile.is_some() {
                    "Edit SSH connection"
                } else {
                    "New SSH connection"
                }
            }
            Dialog::Commands => "Command palette",
            Dialog::Close(_) => "Close this session?",
            Dialog::PasteConfirm { .. } => "Confirm terminal paste",
            Dialog::FileOverwrite(_) => "Replace existing file?",
            Dialog::FileDeleteLocal(_) => "Delete local file?",
            Dialog::FileDeleteRemote { directory, .. } => {
                if *directory {
                    "Delete remote directory?"
                } else {
                    "Delete remote file?"
                }
            }
            Dialog::FileName(file_action) => match file_action {
                FileNameAction::MkdirLocal(_) => "New local folder",
                FileNameAction::MkdirRemote { .. } => "New remote folder",
                FileNameAction::RenameLocal(_) => "Rename local file",
                FileNameAction::RenameRemote { .. } => "Rename remote file",
            },
            Dialog::ImportProfiles => "Import SSH profiles",
            Dialog::Tunnels => "Tunnel",
            Dialog::About => "Inspirum Terminal · Iced",
            Dialog::Exit => "Exit",
        };
        let body: Element<'_, Message> = match dialog {
            Dialog::Tools => self.tools_view(),
            Dialog::Connection => {
                let editing = self.editing_profile.is_some();
                let mut form = column![
                    text(if editing {
                        "Changes are saved to the existing profile. Live panes keep their current connection until reconnect."
                    } else {
                        "Saved through the existing validated, non-secret profile store."
                    })
                    .color(if editing { fg() } else { muted() }),
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
                    .spacing(8),
                    field(
                        "Folder",
                        "Optional group",
                        &self.form.folder,
                        Message::Folder
                    ),
                    row![
                        text("Favourite").size(13).color(fg()),
                        action(
                            if self.form.favorite { "Pinned" } else { "Not pinned" },
                            Message::Favorite(!self.form.favorite)
                        )
                        .style(if self.form.favorite {
                            flat_selected
                        } else {
                            quiet
                        }),
                    ]
                    .spacing(8)
                    .align_y(iced::Center),
                    column![
                        text("Tags").size(13).color(fg()),
                        row![
                            text_input("Add profile tag", &self.form.tag_input)
.style(flat_input)
                                .id("connection-tag")
                                .on_input(Message::TagInput)
                                .on_submit(Message::AddTag)
                                .size(13).padding([3, 6]),
                            action("Add", Message::AddTag),
                        ]
                        .spacing(8)
                        .align_y(iced::Center),
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
                .spacing(8);

                if self.form.advanced {
                    form = form.push(
                        container(
                            column![
                                text("Host key verification").size(11).color(fg()),
                                row![
                                    action(
                                        "Ask on new/changed host",
                                        Message::StrictHostKey(false)
                                    )
                                    .style(if !self.form.strict_host_key {
                                        flat_selected
                                    } else {
                                        quiet
                                    }),
                                    action(
                                        "Known hosts only",
                                        Message::StrictHostKey(true)
                                    )
                                    .style(if self.form.strict_host_key {
                                        flat_selected
                                    } else {
                                        quiet
                                    }),
                                ]
                                .spacing(8),
                                text("Host-key verification is always enabled. “Known hosts only” refuses unknown hosts; “Ask” keeps OpenSSH's interactive trust prompt.")
                                    .size(11)
                                    .color(muted()),
                                field(
                                    "Identity file",
                                    "~/.ssh/id_ed25519 or other key path",
                                    &self.form.identity_file,
                                    Message::IdentityFile
                                ),
                                text("Authentication · Inherit follows SSH config defaults")
                                    .size(11)
                                    .color(muted()),
                                policy_field("Public-key authentication", self.form.public_key_auth, Message::PublicKeyAuth),
                                policy_field("Password authentication", self.form.password_auth, Message::PasswordAuth),
                                policy_field("Keyboard-interactive authentication", self.form.keyboard_interactive_auth, Message::KeyboardInteractiveAuth),
                                policy_field("Kerberos / GSSAPI authentication", self.form.gssapi_auth, Message::GssapiAuth),
                                text("GSSAPI requires Kerberos tickets and a capable system OpenSSH; runtime-verified against an isolated Linux realm, not yet verified on Windows/macOS.")
                                    .size(11)
                                    .color(muted()),
                                text("Session behavior · inherited unless explicitly overridden")
                                    .size(11)
                                    .color(muted()),
                                policy_field("SSH agent forwarding", self.form.agent_forwarding, Message::AgentForwarding),
                                policy_field("X11 forwarding", self.form.x11_forwarding, Message::X11Forwarding),
                                policy_field("Compression", self.form.compression, Message::Compression),
                                text("Only enable agent or X11 forwarding for remote hosts you trust; agent forwarding exposes signing capability to that host.")
                                    .size(11)
                                    .color(muted()),
                                text("X11 requires a local X server/DISPLAY and remote X11 support; it has not been verified in Windows/macOS native CI.")
                                    .size(11)
                                    .color(muted()),
                                text(forwarding_preflight_hint(
                                    self.form.agent_forwarding,
                                    self.form.x11_forwarding,
                                    std::env::var_os("SSH_AUTH_SOCK").is_some_and(|value| !value.is_empty()),
                                    std::env::var_os("DISPLAY").is_some_and(|value| !value.is_empty()),
                                ))
                                    .size(11)
                                    .color(theme::palette().warning_text),

                                field(
                                    "ProxyJump",
                                    "bastion or user@bastion:2222",
                                    &self.form.proxy_jump,
                                    Message::ProxyJump
                                ),
                                text("Structured proxy · mutually exclusive with ProxyJump")
                                    .size(11)
                                    .color(muted()),
                                row![
                                    action(
                                        "None",
                                        Message::ProxyKind(ProxyKind::None)
                                    )
                                    .style(if self.form.proxy_kind == ProxyKind::None {
                                        flat_selected
                                    } else {
                                        quiet
                                    }),
                                    action(
                                        "HTTP CONNECT",
                                        Message::ProxyKind(ProxyKind::HttpConnect)
                                    )
                                    .style(if self.form.proxy_kind == ProxyKind::HttpConnect {
                                        flat_selected
                                    } else {
                                        quiet
                                    }),
                                    action(
                                        "SOCKS5",
                                        Message::ProxyKind(ProxyKind::Socks5)
                                    )
                                    .style(if self.form.proxy_kind == ProxyKind::Socks5 {
                                        flat_selected
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
                                        .spacing(8),
                                        row![
                                            text("Proxy authentication")
                                                .size(12)
                                                .color(muted()),
                                            action(
                                                "None",
                                                Message::ProxyAuth(ProxyAuth::None)
                                            )
                                            .style(if self.form.proxy_auth == ProxyAuth::None {
                                                flat_selected
                                            } else {
                                                quiet
                                            }),
                                            action(
                                                "Environment credentials",
                                                Message::ProxyAuth(ProxyAuth::Environment)
                                            )
                                            .style(if self.form.proxy_auth == ProxyAuth::Environment {
                                                flat_selected
                                            } else {
                                                quiet
                                            }),
                                        ]
                                        .spacing(8)
                                        .align_y(iced::Center),
                                        text("Environment mode reads ephemeral proxy credentials at runtime; usernames/passwords are never saved in the profile.")
                                            .size(11)
                                            .color(muted()),
                                    ]
                                    .spacing(8)
                                    .into()
                                },
                                text("Algorithm policy · leave empty to inherit OpenSSH defaults")
                                    .size(11)
                                    .color(muted()),
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
                                .spacing(8),
                                field(
                                    "Remote command",
                                    "Optional command after authentication",
                                    &self.form.remote_command,
                                    Message::RemoteCommand
                                ),
                                text("Tunnels · OpenSSH forward specs separated by \";\"; started and stopped from the Tunnels panel")
                                    .size(11)
                                    .color(muted()),
                                forward_editor(
                                    "Local forward (-L)",
                                    "127.0.0.1:8080:internal.example:80",
                                    &self.form.local_forwards,
                                    Message::LocalForwards
                                ),
                                forward_editor(
                                    "Remote forward (-R)",
                                    "127.0.0.1:9000:127.0.0.1:3000",
                                    &self.form.remote_forwards,
                                    Message::RemoteForwards
                                ),
                                forward_editor(
                                    "Dynamic forward (-D)",
                                    "127.0.0.1:1080",
                                    &self.form.dynamic_forwards,
                                    Message::DynamicForwards
                                ),
                                text("These values write directly into the existing Session::ssh model and use its current validation. Algorithm fields inherit OpenSSH/config defaults when empty; legacy algorithms still require explicit user input and retain the existing warning policy. Other advanced settings not exposed here yet are preserved when editing an existing profile.")
                                    .size(12)
                                    .color(muted()),
                            ]
                            .spacing(8)
                        )
                        .padding(8)
                        .style(card),
                    );
                }
                if let Some(error) = &self.form.error {
                    form = form.push(text(error).color(danger()));
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
                    .spacing(8),
                )
                .into()
            }
            Dialog::Commands => {
                let items = iced_palette_items(&self.command_query, &self.snippets);
                let mut palette = column![
                    text("Application actions are [APP]. Snippets are [REMOTE TEXT] and only stage text; they never execute automatically.")
                        .size(12)
                        .color(muted()),
                    text_input("Search actions or snippets", &self.command_query)
.style(flat_input)
                        .id("command-query")
                        .on_input(Message::CommandQuery)
                        .size(13).padding([3, 6]),
                ]
                .spacing(8);

                if items.is_empty() {
                    palette = palette.push(text("No matching commands.").size(12).color(muted()));
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
                    text("REUSABLE SNIPPETS").size(11).color(muted()),
                    text_input("Snippet name", &self.snippet_name)
.style(flat_input)
                        .on_input(Message::SnippetName)
                        .size(13).padding([3, 6]),
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
                            text("COMMAND SENDER · FOCUSED PANE").size(11).color(blue()),
                            text_editor(&self.command_content)
                                .placeholder("Remote-shell text to stage")
                                .on_action(Message::CommandEdit)
                                .height(120)
                                .padding(8),
                            row![
                                action("Send explicitly", Message::CommandSend).style(primary),
                                text("Multiline payloads still require the terminal paste confirmation.")
                                    .size(11)
                                    .color(muted()),
                            ]
                            .spacing(8)
                            .align_y(iced::Center),
                        ]
                        .spacing(8)
                    )
                    .padding(8)
                    .style(card),
                    container(snippets).padding(8).style(card),
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
                .spacing(8)
                .into()
            }
            Dialog::Close(index) => column![
                text("This disconnects the live SSH/PTY session and closes its Iced workspace tab.")
                    .color(fg()),
                row![
                    space::horizontal(),
                    action("Cancel", Message::CloseDialog),
                    action("Close session", Message::ConfirmClose(*index)).style(button::danger)
                ]
                .spacing(8),
            ]
            .spacing(8)
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
                    text(format!(
                        "Paste {} bytes across approximately {} line(s)?",
                        prepared.len(),
                        line_count
                    ))
                    .color(fg()),
                    text("Review carefully. Multiline terminal pastes can execute several commands immediately.")
                        .size(13)
                        .color(muted()),
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
                        .color(muted()),
                    container(
                        scrollable(text(preview).font(Font::MONOSPACE).size(13))
                            .height(220)
                    )
                    .padding(8)
                    .width(Fill)
                    .style(card),
                    row![
                        space::horizontal(),
                        action("Cancel", Message::CloseDialog),
                        action("Paste now", Message::ConfirmPaste).style(primary)
                    ]
                    .spacing(8),
                    text("Keyboard: Enter confirms; Escape cancels.")
                        .size(12)
                        .color(muted()),
                ]
                .spacing(8)
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
                    text(format!(
                        "A file already exists at the {direction} destination."
                    ))
                    .color(fg()),
                    container(
                        column![
                            text("Source").size(11).color(fg()),
                            text(source).font(Font::MONOSPACE).size(12),
                            text("Destination").size(11).color(fg()),
                            text(destination).font(Font::MONOSPACE).size(12),
                        ]
                        .spacing(6)
                    )
                    .padding(8)
                    .width(Fill)
                    .style(card),
                    text("Replacing is explicit. The transfer still uses the existing validated SFTP path and verification/resume policy.")
                        .size(12)
                        .color(muted()),
                    row![
                        space::horizontal(),
                        action("Cancel", Message::CloseDialog),
                        action("Replace and transfer", Message::ConfirmFileOverwrite)
                            .style(button::danger)
                    ]
                    .spacing(8),
                ]
                .spacing(8)
                .into()
            }
            Dialog::FileDeleteLocal(path) => column![
                text(path.to_string_lossy().into_owned())
                    .font(Font::MONOSPACE)
                    .size(12),
                text("This action is permanent. Directories are removed only when empty.")
                    .size(12)
                    .color(fg()),
                row![
                    space::horizontal(),
                    action("Cancel", Message::CloseDialog),
                    action("Delete", Message::ConfirmFileDelete).style(button::danger)
                ]
                .spacing(8),
            ]
            .spacing(8)
            .into(),
            Dialog::FileDeleteRemote { path, directory, .. } => column![
                text(path).font(Font::MONOSPACE).size(12),
                text(if *directory {
                    "This action is permanent. Remote directories are removed only when empty."
                } else {
                    "This action is permanent and uses the validated SFTP delete path."
                })
                .size(12)
                .color(fg()),
                row![
                    space::horizontal(),
                    action("Cancel", Message::CloseDialog),
                    action("Delete", Message::ConfirmFileDelete).style(button::danger)
                ]
                .spacing(8),
            ]
            .spacing(8)
            .into(),
            Dialog::FileName(file_action) => {
                let (_title, hint) = match file_action {
                    FileNameAction::MkdirLocal(_) => ("New local folder", "Folder name"),
                    FileNameAction::MkdirRemote { .. } => ("New remote folder", "Folder name"),
                    FileNameAction::RenameLocal(_) => ("Rename local file", "New filename"),
                    FileNameAction::RenameRemote { .. } => ("Rename remote file", "New filename"),
                };
                column![
                    text_input(hint, &self.files.name_input)
.style(flat_input)
                        .id("file-name")
                        .on_input(Message::FilesNameChanged)
                        .on_submit(Message::ConfirmFileNameAction)
                        .size(13).padding([3, 6]),
                    text("Names are validated by the existing SFTP/local-file policy before any operation is executed.")
                        .size(12)
                        .color(muted()),
                    row![
                        space::horizontal(),
                        action("Cancel", Message::CloseDialog),
                        action("Apply", Message::ConfirmFileNameAction).style(primary)
                    ]
                    .spacing(8),
                ]
                .spacing(8)
                .into()
            }
            Dialog::ImportProfiles => column![
                text("Merge a validated non-secret Inspirum profile JSON file into the current library. Existing profiles are never overwritten by import.")
                    .color(fg()),
                column![
                    text("Import file").size(13).color(theme::palette().text),
                    text_input("/path/to/profiles.json", &self.import_path)
.style(flat_input)
                        .id("import-path")
                        .on_input(Message::ImportPath)
                        .on_submit(Message::ConfirmImportProfiles)
                        .size(13).padding([3, 6])
                ]
                .spacing(7),
                text("Conflicting folder/name entries, invalid SSH policy, secret-bearing unsupported fields, oversized files, and malformed JSON are rejected before the active profile store is changed.")
                    .size(12)
                    .color(muted()),
                row![
                    space::horizontal(),
                    action("Cancel", Message::CloseDialog),
                    action("Import and merge", Message::ConfirmImportProfiles).style(primary),
                ]
                .spacing(8),
            ]
            .spacing(8)
            .into(),
            Dialog::Tunnels => self.tunnels_view(),
            Dialog::About => column![
                text("Inspirum Terminal uses Iced for the desktop interface and the shared OpenSSH/PTY backend for terminal sessions.\n\nTools includes profiles and startup, tabs and saved split workspaces, synchronized input, history and logging, terminal appearance and pointer preferences, SSH trust and multiplexing, forwarding, tmux, SCP, snippets and sanitized support reports.\n\nThe Files pane provides graphical SFTP transfers and remote editing. Clipboard and command sends retain explicit confirmation and terminal identity checks.")
                    .color(muted()),
                action("Back to workspace", Message::CloseDialog).style(primary),
            ]
            .spacing(8)
            .into(),
            Dialog::Exit => column![
                text(exit_confirm_body(self.tabs.len())).color(fg()),
                row![
                    space::horizontal(),
                    action("Cancel", Message::CloseDialog),
                    action("Exit", Message::ConfirmExit).style(button::danger),
                ]
                .spacing(8),
            ]
            .spacing(8)
            .into(),
        };

        // Keep form controls inside the scroll rail, not underneath it. The
        // gutter belongs to the scrollable content so it survives scrolling.
        let body = if matches!(dialog, Dialog::Connection) {
            scrollable(container(body).padding(iced::Padding {
                top: 0.0,
                right: 16.0,
                bottom: 0.0,
                left: 0.0,
            }))
        } else {
            scrollable(body)
        };
        dialog_frame(title, container(body).padding(12))
            .width(if matches!(dialog, Dialog::Tools) {
                880
            } else {
                600
            })
            .max_height(720)
            .into()
    }

    /// Exit confirmation on views that otherwise hide dialogs (privacy curtain,
    /// focus mode). The workspace underneath is not the normal chrome.
    fn cover_with_exit_confirmation<'a>(
        &'a self,
        under: Element<'a, Message>,
    ) -> Element<'a, Message> {
        let Some(dialog) = &self.dialog else {
            return under;
        };
        if !matches!(dialog, Dialog::Exit) {
            return under;
        }
        let content = self.dialog_view(dialog);
        stack![
            under,
            opaque(
                mouse_area(center(opaque(content)).style(|_| container::Style {
                    background: Some(Color::from_rgba(0.0, 0.0, 0.0, 0.35).into()),
                    ..Default::default()
                }))
                .on_press(Message::CloseDialog)
            ),
        ]
        .into()
    }

    fn close_find_strip(&mut self) {
        self.find_open = false;
        self.find_focused = false;
    }

    fn restore_find_focus(&self) -> Task<Message> {
        if self.find_open && self.find_focused && self.dialog.is_none() && !self.privacy_locked {
            operation::focus(FIND_INPUT_ID)
        } else {
            Task::none()
        }
    }

    /// Point History search at the focused pane the same way opening the
    /// History panel does, then show the strip and focus its input.
    fn open_find_strip(&mut self) -> Task<Message> {
        if self.dialog.is_some() || self.remote_editor.is_some() {
            return Task::none();
        }
        if self.focus_mode {
            self.status = "Exit focus mode before searching history.".into();
            return Task::none();
        }
        let Some(id) = self.tabs.get(self.active).and_then(|tab| {
            tab.panes
                .get(tab.focus)
                .filter(|pane| !pane.exited)
                .map(|pane| pane.id)
        }) else {
            self.status = "Focus a terminal pane before searching history.".into();
            return Task::none();
        };
        // history_refresh only records an owner when a live terminal exists.
        // Set it first so a not-yet-started pane still owns the strip.
        self.tools.history_owner = Some(id);
        self.history_refresh();
        self.find_open = true;
        self.find_focused = true;
        operation::focus(FIND_INPUT_ID)
    }

    fn find_strip(&self) -> Element<'_, Message> {
        let p = theme::palette();
        let state = self
            .tools
            .history_owner
            .and_then(|id| self.tools.histories.get(&id));
        let query = state.map(|state| state.query()).unwrap_or("");
        let (counter, danger) = find_counter(
            query,
            state.map(|state| state.selected_hit_index()).unwrap_or(0),
            state.map(|state| state.hits().len()).unwrap_or(0),
        );
        let counter_color = if danger { p.danger } else { p.text };
        let chrome = |label: String| {
            text(label)
                .size(theme::metrics::UI_TEXT)
                .color(p.text)
                .font(Font::MONOSPACE)
                .wrapping(iced::widget::text::Wrapping::None)
        };
        let counter_glyph = |label: String, color: Color| {
            text(label)
                .size(theme::metrics::UI_TEXT)
                .color(color)
                .font(Font::MONOSPACE)
                .wrapping(iced::widget::text::Wrapping::None)
        };
        // Seven monospace columns (`123/456`) so Prev/Next do not jump.
        let counter_slot = stack![
            counter_glyph(FIND_COUNTER_MIN.to_owned(), Color::TRANSPARENT),
            counter_glyph(counter, counter_color),
        ];
        let body = row![
            chrome("Find".to_owned()),
            space::horizontal().width(6),
            text_input("", query)
                .id(FIND_INPUT_ID)
                .style(flat_input)
                .on_input(Message::FindQuery)
                .on_submit(Message::FindNext)
                .font(Font::MONOSPACE)
                .size(theme::metrics::UI_TEXT)
                .padding([2, 6])
                .width(Fill),
            space::horizontal().width(8),
            counter_slot,
            space::horizontal().width(10),
            action("Prev", Message::FindPrev),
            space::horizontal().width(6),
            action("Next", Message::FindNext),
            space::horizontal().width(8),
            // Dock strip close: 12 px icon, 4 px padding (20 px hit), normal text.
            button(icons::icon(icons::Icon::Close, 12.0, p.text))
                .padding(4)
                .style(icon_button)
                .on_press(Message::FindClose),
        ]
        .spacing(0)
        .padding([0, 8])
        .height(theme::metrics::STRIP_HEIGHT - 1.0)
        .align_y(iced::Center);
        // Toolbar background token (surface -> palette.chrome), 1 px top rule.
        container(column![hline(), body])
            .width(Fill)
            .style(surface)
            .into()
    }
}

/// Width reservation for the Find counter (`123/456` is seven columns).
const FIND_COUNTER_MIN: &str = "123/456";

/// Counter label, and whether it uses the danger colour.
/// Danger only when `query` is non-empty and there are no hits.
fn find_counter(query: &str, selected_index: usize, hit_count: usize) -> (String, bool) {
    if hit_count == 0 {
        return ("0/0".to_owned(), !query.is_empty());
    }
    (format!("{}/{}", selected_index + 1, hit_count), false)
}

fn exit_confirm_body(open_sessions: usize) -> String {
    if open_sessions == 1 {
        "1 session is open. Exit and disconnect it?".to_owned()
    } else {
        format!("{open_sessions} sessions are open. Exit and disconnect them?")
    }
}

fn truncate(value: &str, max: usize) -> String {
    if value.chars().count() <= max {
        value.to_owned()
    } else {
        let mut out: String = value.chars().take(max.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}

fn forward_editor<'a>(
    label: &'a str,
    hint: &'a str,
    value: &'a str,
    message: fn(String) -> Message,
) -> Element<'a, Message> {
    column![
        text(label).size(13).color(muted()),
        text_input(hint, value)
            .style(flat_input)
            .on_input(message)
            .padding(8)
            .size(13),
    ]
    .spacing(4)
    .into()
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
        text(label).size(13).color(theme::palette().text),
        text_input(hint, value)
            .style(flat_input)
            .id(id)
            .on_input(message)
            .on_submit(Message::Submit)
            .size(13)
            .padding([3, 6])
    ]
    .spacing(4)
    .into()
}

fn policy_field<'a>(
    label: &'a str,
    value: Option<bool>,
    message: fn(Option<bool>) -> Message,
) -> Element<'a, Message> {
    column![
        text(label).size(12).color(theme::palette().text),
        row![
            action("Inherit", message(None)).style(if value.is_none() {
                flat_selected
            } else {
                quiet
            }),
            action("Enable", message(Some(true))).style(if value == Some(true) {
                flat_selected
            } else {
                quiet
            }),
            action("Disable", message(Some(false))).style(if value == Some(false) {
                flat_selected
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
    button(text(label.into()).size(13))
        .padding([3, 10])
        .style(quiet)
        .on_press(message)
}

fn surface(_: &Theme) -> container::Style {
    let p = theme::palette();
    container::Style {
        background: Some(p.chrome.into()),
        text_color: Some(p.text),
        ..Default::default()
    }
}

fn card(_: &Theme) -> container::Style {
    let p = theme::palette();
    container::Style {
        background: Some(p.dock.into()),
        text_color: Some(p.text),
        border: Border {
            color: p.border,
            width: 1.0,
            radius: 0.0.into(),
        },
        ..Default::default()
    }
}

fn active_card(theme: &Theme) -> container::Style {
    container::Style {
        border: Border {
            color: blue(),
            width: 1.0,
            radius: 0.0.into(),
        },
        ..card(theme)
    }
}

/// Flat WindTerm text button: `list` fill with a 1px `border` outline at rest,
/// so a control still reads as clickable before the pointer reaches it.
fn menu_title(_: &Theme, status: button::Status, open: bool) -> button::Style {
    let p = theme::palette();
    let background = match status {
        button::Status::Hovered | button::Status::Pressed => Some(p.hover.into()),
        _ if open => Some(p.hover.into()),
        _ => None,
    };
    button::Style {
        background,
        text_color: p.text,
        ..Default::default()
    }
}

fn menu_item(_: &Theme, status: button::Status) -> button::Style {
    let p = theme::palette();
    button::Style {
        background: matches!(status, button::Status::Hovered | button::Status::Pressed)
            .then(|| p.hover.into()),
        text_color: p.text,
        ..Default::default()
    }
}

fn quiet(_: &Theme, status: button::Status) -> button::Style {
    let p = theme::palette();
    let (background, border_color, text_color) = match status {
        button::Status::Hovered => (p.hover, p.accent, p.text),
        button::Status::Pressed => (p.selected_row, p.accent, p.text),
        button::Status::Disabled => (p.dock, p.border, p.muted),
        button::Status::Active => (p.list, p.border, p.text),
    };
    button::Style {
        background: Some(background.into()),
        text_color,
        border: Border {
            color: border_color,
            width: 1.0,
            radius: 2.0.into(),
        },
        ..Default::default()
    }
}

fn selected_button(theme: &Theme, status: button::Status) -> button::Style {
    let base = quiet(theme, status);
    let p = theme::palette();
    button::Style {
        background: Some(p.selected_row.into()),
        text_color: p.text,
        border: Border {
            color: p.accent,
            ..base.border
        },
        ..base
    }
}

fn primary(theme: &Theme, status: button::Status) -> button::Style {
    let base = quiet(theme, status);
    let p = theme::palette();
    button::Style {
        background: Some(p.accent.into()),
        // Both chrome accents are mid-value blues; dark lettering clears AA.
        text_color: Color::BLACK,
        border: Border {
            color: p.accent,
            ..base.border
        },
        ..base
    }
}

fn hline<'a>() -> Element<'a, Message> {
    container(space::horizontal())
        .width(Fill)
        .height(1)
        .style(border_fill)
        .into()
}

fn compact_action<'a>(label: impl Into<String>, message: Message) -> button::Button<'a, Message> {
    button(
        text(label.into())
            .size(12)
            .wrapping(iced::widget::text::Wrapping::None),
    )
    .padding([2, 6])
    .style(icon_button)
    .on_press(message)
}

fn tool_icon<'a>(icon: icons::Icon, tip: &'a str, message: Message) -> Element<'a, Message> {
    let p = theme::palette();
    iced::widget::tooltip(
        button(icons::icon(icon, 14.0, p.text))
            .padding(6)
            .style(icon_button)
            .on_press(message),
        container(text(tip).size(theme::metrics::STATUS_TEXT))
            .padding([2, 6])
            .style(card),
        iced::widget::tooltip::Position::Bottom,
    )
    .into()
}

fn fill_style(color: Color) -> container::Style {
    container::Style {
        background: Some(color.into()),
        text_color: Some(theme::palette().text),
        ..Default::default()
    }
}

fn list_area(_: &Theme) -> container::Style {
    fill_style(theme::palette().list)
}

fn border_fill(_: &Theme) -> container::Style {
    fill_style(theme::palette().border)
}

fn accent_fill(_: &Theme) -> container::Style {
    fill_style(theme::palette().accent)
}

fn tab_active_fill(_: &Theme) -> container::Style {
    fill_style(theme::palette().tab_active)
}

fn tab_inactive_fill(_: &Theme) -> container::Style {
    fill_style(theme::palette().tab_inactive)
}

fn pane_header_fill(_: &Theme) -> container::Style {
    fill_style(theme::palette().header)
}

fn pane_body(_: &Theme) -> container::Style {
    fill_style(theme::palette().dock)
}

fn breadcrumb_field(_: &Theme) -> container::Style {
    let p = theme::palette();
    container::Style {
        border: Border {
            color: p.border,
            width: 1.0,
            radius: 0.0.into(),
        },
        ..fill_style(p.raised)
    }
}

/// Flat icon/compact control: no fill or outline at rest, square hover.
fn icon_button(_: &Theme, status: button::Status) -> button::Style {
    let p = theme::palette();
    let (background, text_color) = match status {
        button::Status::Hovered => (Some(p.hover.into()), p.text),
        button::Status::Pressed => (Some(p.selected_row.into()), p.text),
        button::Status::Disabled => (None, p.muted),
        button::Status::Active => (None, p.text),
    };
    button::Style {
        background,
        text_color,
        border: Border::default(),
        ..Default::default()
    }
}

/// Tab label: transparent so the tab container fill shows through.
fn tab_button(theme: &Theme, status: button::Status) -> button::Style {
    button::Style {
        background: None,
        ..icon_button(theme, status)
    }
}

fn file_row(_theme: &Theme, status: button::Status) -> button::Style {
    let palette = theme::palette();
    let background = match status {
        button::Status::Hovered | button::Status::Pressed => palette.hover,
        _ => palette.list,
    };
    button::Style {
        background: Some(background.into()),
        text_color: palette.text_dock,
        border: Border::default(),
        ..button::Style::default()
    }
}

fn file_row_selected(_theme: &Theme, _status: button::Status) -> button::Style {
    let palette = theme::palette();
    button::Style {
        background: Some(palette.selected_row.into()),
        text_color: palette.text_dock,
        border: Border::default(),
        ..button::Style::default()
    }
}

fn file_header(_theme: &Theme) -> container::Style {
    container::Style::default().background(theme::palette().header)
}

fn file_divider(_theme: &Theme) -> container::Style {
    container::Style::default().background(theme::palette().border)
}

fn session_panel(_theme: &Theme) -> container::Style {
    fill_style(theme::palette().list)
}

/// Borderless WindTerm filter field; the underline is a separate hline.
fn session_filter(_theme: &Theme, status: text_input::Status) -> text_input::Style {
    let p = theme::palette();
    text_input::Style {
        background: Color::TRANSPARENT.into(),
        border: Border::default(),
        icon: p.muted,
        placeholder: p.muted,
        value: p.text_dock,
        selection: match status {
            text_input::Status::Disabled => p.hover,
            _ => p.selected_row,
        },
    }
}

fn session_row(_theme: &Theme, status: button::Status) -> button::Style {
    let p = theme::palette();
    let background = match status {
        button::Status::Hovered | button::Status::Pressed => Some(p.hover.into()),
        _ => None,
    };
    button::Style {
        background,
        text_color: p.text_dock,
        border: Border::default(),
        ..button::Style::default()
    }
}

fn session_row_open(_theme: &Theme, _status: button::Status) -> button::Style {
    let p = theme::palette();
    button::Style {
        background: Some(p.selected_row.into()),
        text_color: p.text_dock,
        border: Border::default(),
        ..button::Style::default()
    }
}

fn flat_input(_: &Theme, status: text_input::Status) -> text_input::Style {
    let p = theme::palette();
    let focused = matches!(status, text_input::Status::Focused { .. });
    text_input::Style {
        background: p.list.into(),
        border: Border {
            color: if focused { p.accent } else { p.border },
            width: 1.0,
            radius: 0.0.into(),
        },
        icon: p.muted,
        placeholder: p.muted,
        value: p.text,
        selection: p.selected_row,
    }
}

fn dialog_strip(_: &Theme) -> container::Style {
    container::Style::default().background(theme::palette().chrome)
}

fn dialog_window(_: &Theme) -> container::Style {
    let p = theme::palette();
    container::Style {
        background: Some(p.dock.into()),
        text_color: Some(p.text),
        border: Border {
            color: p.border,
            width: 1.0,
            radius: 0.0.into(),
        },
        ..Default::default()
    }
}

/// WindTerm-style dialog chrome: dock fill, 1px border, chrome title strip.
fn dialog_frame<'a>(
    title: &'a str,
    body: impl Into<Element<'a, Message>>,
) -> container::Container<'a, Message> {
    container(column![
        container(text(title).size(13).color(theme::palette().text))
            .padding([0, 8])
            .align_y(iced::Center)
            .width(iced::Length::Fill)
            .height(theme::metrics::STRIP_HEIGHT)
            .style(dialog_strip),
        hline(),
        body.into(),
    ])
    .padding(1)
    .style(dialog_window)
}

/// Selected dialog choice: a visible 2px accent outline even on hover.
fn flat_selected(_: &Theme, status: button::Status) -> button::Style {
    let p = theme::palette();
    let background = match status {
        button::Status::Hovered => p.hover,
        _ => p.selected_row,
    };
    button::Style {
        background: Some(background.into()),
        text_color: p.text,
        border: Border {
            color: p.selection_indicator,
            width: 2.0,
            radius: 2.0.into(),
        },
        ..Default::default()
    }
}

/// A relative remote destination (`.`) qualified with the session target.
fn relative_remote_destination(session: &Session) -> String {
    match (session.user.trim(), session.host.trim()) {
        (_, "") => ".".to_owned(),
        ("", host) => format!("{host}:."),
        (user, host) => format!("{user}@{host}:."),
    }
}

/// `(name, verb, DIRECTION, destination dir)` for a transfer block (spec 5.3).
fn transfer_job_summary(job: &TransferJob) -> (String, &'static str, &'static str, String) {
    let local_name = job
        .pending
        .local
        .file_name()
        .map(|value| value.to_string_lossy().into_owned())
        .unwrap_or_else(|| job.pending.local.to_string_lossy().into_owned());
    let remote_name = job
        .pending
        .remote
        .rsplit('/')
        .next()
        .unwrap_or(job.pending.remote.as_str())
        .to_owned();
    match job.pending.direction {
        TransferDirection::Upload => {
            let dir = job
                .pending
                .remote
                .rsplit_once('/')
                .map(|(dir, _)| if dir.is_empty() { "/" } else { dir })
                .map(str::to_owned)
                .unwrap_or_else(|| relative_remote_destination(&job.pending.session));
            (local_name, "uploaded", "UPLOAD", dir)
        }
        TransferDirection::Download => {
            let dir = job
                .pending
                .local
                .parent()
                .map(|dir| dir.to_string_lossy().into_owned())
                .unwrap_or_default();
            (remote_name, "downloaded", "DOWNLOAD", dir)
        }
    }
}

/// Path bar (34px): underlined plain-text path input then flat buttons.
fn files_path_bar<'a>(
    input: Element<'a, Message>,
    go: button::Button<'a, Message>,
    up_message: Message,
) -> Element<'a, Message> {
    let p = theme::palette();
    // Icon + label: CI OCR clicks the literal `Up`.
    let up = iced::widget::tooltip(
        button(
            row![
                icons::icon(icons::Icon::ArrowUp, 12.0, p.text),
                text("Up").size(12)
            ]
            .spacing(3)
            .align_y(iced::Center),
        )
        .padding([2, 6])
        .style(icon_button)
        .on_press(up_message),
        container(text("Parent directory").size(theme::metrics::STATUS_TEXT))
            .padding([2, 6])
            .style(card),
        iced::widget::tooltip::Position::Bottom,
    );
    row![
        column![
            input,
            container(space::horizontal())
                .height(1)
                .width(Fill)
                .style(border_fill),
        ]
        .width(Fill),
        up,
        tool_icon(icons::Icon::Refresh, "Refresh", Message::FilesRefresh),
        go,
    ]
    .spacing(4)
    .padding([0, 6])
    .height(34)
    .align_y(iced::Center)
    .into()
}

fn files_action_row(buttons: Vec<Element<'_, Message>>) -> Element<'_, Message> {
    scrollable(
        row(buttons)
            .spacing(6)
            .padding([2, 6])
            .align_y(iced::Center),
    )
    .direction(scrollable::Direction::Horizontal(
        scrollable::Scrollbar::new().width(2).scroller_width(2),
    ))
    .height(30)
    .into()
}

fn files_path_input(_: &Theme, status: text_input::Status) -> text_input::Style {
    let p = theme::palette();
    text_input::Style {
        background: Color::TRANSPARENT.into(),
        border: Border::default(),
        icon: p.muted,
        placeholder: p.muted,
        value: p.text,
        selection: match status {
            text_input::Status::Disabled => p.hover,
            _ => p.selected_row,
        },
    }
}

fn transfer_progress(_: &Theme) -> progress_bar::Style {
    let p = theme::palette();
    progress_bar::Style {
        background: Color::TRANSPARENT.into(),
        bar: p.accent.into(),
        border: Border {
            color: p.accent,
            width: 1.0,
            radius: 1.0.into(),
        },
    }
}

/// Selected session row: palette `selected_row` fill, full width, no border.
fn session_row_selected(_theme: &Theme, _status: button::Status) -> button::Style {
    let p = theme::palette();
    button::Style {
        background: Some(p.selected_row.into()),
        text_color: p.text_dock,
        border: Border::default(),
        ..button::Style::default()
    }
}

const SESSION_DOUBLE_CLICK: Duration = Duration::from_millis(400);

/// True when `now` is at most [`SESSION_DOUBLE_CLICK`] after `previous`.
fn session_double_click(previous: Instant, now: Instant) -> bool {
    now.saturating_duration_since(previous) <= SESSION_DOUBLE_CLICK
}

/// Collapsed groups hide their rows only while the filter is empty.
/// A query that matched a session (the caller already filtered) shows it.
fn session_group_hides_rows(collapsed: bool, query: &str) -> bool {
    collapsed && query.trim().is_empty()
}

#[cfg(test)]
mod tests {
    #[test]
    fn clickable_controls_are_distinct_from_labels_at_rest() {
        use iced::widget::button::Status;
        let theme = iced::Theme::Dark;
        for status in [
            Status::Active,
            Status::Hovered,
            Status::Pressed,
            Status::Disabled,
        ] {
            let style = super::quiet(&theme, status);
            assert!(style.background.is_some(), "{status:?} needs a fill");
            assert!(style.border.width >= 1.0, "{status:?} needs an outline");
        }
        let rest = super::quiet(&theme, Status::Active);
        let hover = super::quiet(&theme, Status::Hovered);
        assert_ne!(rest.border.color, hover.border.color);
        let selected = super::selected_button(&theme, Status::Active);
        assert_eq!(selected.border.color, super::blue());
        assert_eq!(selected.text_color, super::fg());
        assert_eq!(
            super::primary(&theme, Status::Active).text_color,
            iced::Color::WHITE
        );
    }

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
    fn file_listing_summary_uses_singular_for_one_item() {
        assert_eq!(
            file_listing_summary(1, 2048),
            format!("1 item {}", windterm_size(2048))
        );
        assert!(file_listing_summary(0, 0).starts_with("0 items "));
        assert!(file_listing_summary(3, 10).starts_with("3 items "));
    }

    #[test]
    fn dock_splitter_drag_resizes_right_dock() {
        let path = std::env::temp_dir().join(format!(
            "inspirum-iced-dock-drag-{}-nonexistent.json",
            std::process::id()
        ));
        let mut app = App::boot(path, None);
        app.ui_state_path = None;
        let _ = app.update(Message::DockMove(dock::Panel::Filer, dock::Area::Right));
        assert!(app.docks.right_width >= dock::FILE_PANEL_MIN_WIDTH);
        let w = app.window_size.width;
        let _ = app.update(Message::DockDragStart(dock::Area::Right));
        assert_eq!(app.dock_drag, Some(dock::Area::Right));
        let _ = app.update(Message::DockDragMove(iced::Point::new(w - 500.0, 300.0)));
        assert!((app.docks.right_width - 500.0).abs() <= 1.0);
        let _ = app.update(Message::DockDragEnd);
        assert_eq!(app.dock_drag, None);
    }

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
    fn visible_url_highlighting_is_pure_and_trimmed() {
        let spans = |input: &str| {
            let cells: Vec<terminal_core::DisplayCell> = input
                .chars()
                .enumerate()
                .map(|(column, character)| terminal_core::DisplayCell {
                    character,
                    row: 0,
                    column,
                    foreground: [255; 3],
                    background: [0; 3],
                    cursor_color: [255; 3],
                    bold: false,
                    italic: false,
                    underline: false,
                    strikeout: false,
                    wide: false,
                    cursor: false,
                })
                .collect();
            visible_url_ranges(&cells)
        };
        assert_eq!(
            spans("Visit https://example.org/path?q=1, thanks"),
            vec![(6, 34)]
        );
        assert_eq!(spans("http://localhost:8080"), vec![(0, 21)]);
        assert_eq!(
            spans("badhttps://example.org and https://ok.io."),
            vec![(27, 40)]
        );
        assert!(spans("http:// https://").is_empty());
        assert!(spans("ftp://example.org").is_empty());
        assert_eq!(spans("(https://site.test)."), vec![(1, 18)]);
    }

    #[test]
    fn delimiter_highlighting_matches_nested_pairs_and_never_guesses() {
        fn snapshot(input: &str, cursor: usize) -> terminal_core::DisplaySnapshot {
            let cells: Vec<_> = input
                .chars()
                .enumerate()
                .map(|(column, character)| terminal_core::DisplayCell {
                    character,
                    row: 0,
                    column,
                    foreground: [255; 3],
                    background: [0; 3],
                    cursor_color: [255; 3],
                    bold: false,
                    italic: false,
                    underline: false,
                    strikeout: false,
                    wide: false,
                    cursor: column == cursor,
                })
                .collect();
            let length = cells.len();
            terminal_core::DisplaySnapshot {
                rows: 1,
                columns: length,
                background: [0; 3],
                row_ranges: vec![(0, length)],
                cells,
            }
        }
        assert_eq!(matched_delimiters(&snapshot("([x])", 0)), Some((0, 4)));
        assert_eq!(matched_delimiters(&snapshot("([x])", 1)), Some((1, 3)));
        assert_eq!(matched_delimiters(&snapshot("([x])", 4)), Some((0, 4)));
        assert_eq!(matched_delimiters(&snapshot("(a(b)c)", 2)), Some((2, 4)));
        assert_eq!(matched_delimiters(&snapshot("(a(b)c)", 6)), Some((0, 6)));
        assert_eq!(matched_delimiters(&snapshot("(broken", 0)), None);
        assert_eq!(matched_delimiters(&snapshot("plain text", 0)), None);
    }

    #[test]
    fn free_type_draft_never_writes_without_explicit_send() {
        let path = std::env::temp_dir().join(format!(
            "inspirum-free-type-{}-missing.json",
            std::process::id()
        ));
        let mut app = App::boot(path, None);
        let profile = Session {
            name: "free-type test".into(),
            host: "example.invalid".into(),
            ..Session::default()
        };
        let pane = app.new_terminal_pane(profile.clone());
        let id = pane.id;
        app.tabs.push(Workspace::new(profile, pane));

        let _ = app.update(Message::ToggleFreeType(id));
        assert!(app.free_type.contains_key(&id));
        assert!(!app.command_terminal(
            id,
            terminal_core::BackendCommand::Write(b"no remote output".to_vec())
        ));
        let _ = app.update(Message::ClipboardRead(
            id,
            vec![id],
            Some("unapproved paste".into()),
        ));
        assert!(
            app.dialog.is_none(),
            "clipboard cannot bypass local editing"
        );
        let _ = app.update(Message::ToggleLocalNavigation);
        assert!(!app.local_navigation.contains(&id));

        app.free_type.insert(
            id,
            text_editor::Content::with_text("first line\nsecond line"),
        );
        let _ = app.update(Message::FreeTypeSend(id));
        assert!(matches!(app.dialog, Some(Dialog::PasteConfirm { .. })));
        assert_eq!(app.free_type_confirm, Some(id));
        // Even explicit confirmation must reject a draft that changed since
        // the preview was constructed.
        app.free_type.insert(
            id,
            text_editor::Content::with_text("modified after preview"),
        );
        let _ = app.update(Message::ConfirmPaste);
        assert!(app.free_type.contains_key(&id));
        assert!(app.status.contains("cancelled"));
        assert!(app.dialog.is_none());

        let _ = app.update(Message::FreeTypeDiscard(id));
        assert!(!app.free_type.contains_key(&id));
        assert_eq!(app.free_type_confirm, None);
    }

    #[test]
    fn free_type_editors_have_distinct_stable_keyboard_focus_ids() {
        assert_eq!(free_type_editor_id(1), "free-type-1");
        assert_eq!(free_type_editor_id(123), "free-type-123");
        assert_ne!(free_type_editor_id(1), free_type_editor_id(2));
    }

    #[test]
    fn synchronized_write_never_retargets_when_focused_source_is_unavailable() {
        let mut attempts = Vec::new();
        let sent = App::write_source_then_mirrors(20, &[10, 20, 30], |id| {
            attempts.push(id);
            id != 20
        });
        assert!(!sent);
        assert_eq!(
            attempts,
            vec![20],
            "no mirrored PTY may receive failed source input"
        );

        attempts.clear();
        let sent = App::write_source_then_mirrors(20, &[10, 20, 30], |id| {
            attempts.push(id);
            true
        });
        assert!(sent);
        assert_eq!(
            attempts,
            vec![20, 10, 30],
            "focused source must be written first"
        );
    }

    #[test]
    fn remote_keys_are_explicit_opt_in_and_encode_per_pty_cursor_mode() {
        let shortcut = keyboard::Key::Character("m".into());
        let cmd = if cfg!(target_os = "macos") {
            keyboard::Modifiers::LOGO
        } else {
            keyboard::Modifiers::CTRL
        };
        assert!(remote_keys_chord(
            &shortcut,
            cmd | keyboard::Modifiers::SHIFT
        ));
        assert!(!remote_keys_chord(&shortcut, cmd));
        assert_eq!(
            remote_vim_key(
                &keyboard::Key::Character("h".into()),
                keyboard::Modifiers::empty()
            ),
            Some(RemoteCursorKey::Left)
        );
        assert_eq!(
            remote_vim_key(
                &keyboard::Key::Character("j".into()),
                keyboard::Modifiers::empty()
            ),
            Some(RemoteCursorKey::Down)
        );
        assert_eq!(
            remote_vim_key(
                &keyboard::Key::Character("k".into()),
                keyboard::Modifiers::empty()
            ),
            Some(RemoteCursorKey::Up)
        );
        assert_eq!(
            remote_vim_key(
                &keyboard::Key::Character("l".into()),
                keyboard::Modifiers::empty()
            ),
            Some(RemoteCursorKey::Right)
        );
        assert_eq!(
            remote_vim_key(
                &keyboard::Key::Character("h".into()),
                keyboard::Modifiers::CTRL
            ),
            None,
            "modifier chords must not accidentally navigate an SSH PTY"
        );
        assert_eq!(
            remote_cursor_bytes(RemoteCursorKey::Left, terminal_core::TerminalMode::empty()),
            b"\x1b[D"
        );
        assert_eq!(
            remote_cursor_bytes(
                RemoteCursorKey::Left,
                terminal_core::TerminalMode::APP_CURSOR
            ),
            b"\x1bOD",
            "application cursor mode must be encoded per destination"
        );
        assert_eq!(
            remote_cursor_bytes(RemoteCursorKey::Home, terminal_core::TerminalMode::empty()),
            b"\x1b[H"
        );
    }

    #[test]
    fn remote_keys_block_unfocused_disconnected_locked_and_draft_panes() {
        let path = std::env::temp_dir().join(format!(
            "inspirum-remote-keys-{}-nonexistent.json",
            std::process::id()
        ));
        let mut app = App::boot(path, None);
        let profile = Session {
            name: "remote keys fixture".into(),
            host: "example.invalid".into(),
            ..Session::default()
        };
        let first = app.new_terminal_pane(profile.clone());
        let first_id = first.id;
        app.tabs.push(Workspace::new(profile.clone(), first));
        let second = app.new_terminal_pane(profile);
        let second_id = second.id;
        app.tabs[0].split(pane_grid::Axis::Vertical, second);
        assert!(!app.send_remote_cursor(first_id, RemoteCursorKey::Left));
        let _ = app.update(Message::ToggleRemoteKeys(first_id));
        assert!(
            !app.remote_keys.contains(&first_id),
            "unfocused pane cannot enter keys mode"
        );
        let _ = app.update(Message::ToggleRemoteKeys(second_id));
        assert!(app.remote_keys.contains(&second_id));
        assert!(
            !app.send_remote_cursor(second_id, RemoteCursorKey::Left),
            "no backend"
        );
        let _ = app.update(Message::ToggleFreeType(second_id));
        assert!(
            !app.free_type.contains_key(&second_id),
            "remote and draft modes exclude each other"
        );
        let _ = app.update(Message::ToggleLocalNavigation);
        assert!(
            !app.local_navigation.contains(&second_id),
            "remote/local modes exclude each other"
        );
        let _ = app.update(Message::TogglePrivacyLock);
        let _ = app.update(Message::ToggleRemoteKeys(second_id));
        assert!(
            app.remote_keys.contains(&second_id),
            "privacy curtain blocks mode changes"
        );
        assert!(!app.send_remote_cursor(second_id, RemoteCursorKey::Left));
        let _ = app.update(Message::TogglePrivacyLock);
        let _ = app.update(Message::ClosePane(app.tabs[0].focus));
        assert!(
            !app.remote_keys.contains(&second_id),
            "closed pane must not remain a key target"
        );
    }

    #[test]
    fn free_type_shortcut_requires_explicit_command_shift_chord() {
        let key = keyboard::Key::Character("e".into());
        let command = if cfg!(target_os = "macos") {
            keyboard::Modifiers::LOGO
        } else {
            keyboard::Modifiers::CTRL
        };
        assert!(free_type_chord(&key, command | keyboard::Modifiers::SHIFT));
        assert!(!free_type_chord(&key, command));
        assert!(!free_type_chord(&key, keyboard::Modifiers::SHIFT));
    }

    #[test]
    fn direct_pane_sync_selection_requires_two_targets_and_disarms_on_target_change() {
        let path = std::env::temp_dir().join(format!(
            "inspirum-sync-ui-{}-missing.json",
            std::process::id()
        ));
        let mut app = App::boot(path, None);
        let profile = Session {
            name: "fake-disconnected-sync".into(),
            host: "example.invalid".into(),
            ..Session::default()
        };
        let first = app.new_terminal_pane(profile.clone());
        let first_id = first.id;
        app.tabs.push(Workspace::new(profile.clone(), first));
        let second = app.new_terminal_pane(profile);
        let second_id = second.id;
        app.tabs[0].split(pane_grid::Axis::Vertical, second);

        let _ = app.update(Message::Tool(tools::Action::SyncTarget(first_id, true)));
        let _ = app.update(Message::Tool(tools::Action::SyncArm(true)));
        assert!(!app.tools.sync.armed());
        let _ = app.update(Message::Tool(tools::Action::SyncTarget(second_id, true)));
        let _ = app.update(Message::Tool(tools::Action::SyncArm(true)));
        assert!(app.tools.sync.armed());
        assert_eq!(app.tools.sync.destinations(first_id), vec![second_id]);

        let _ = app.update(Message::Tool(tools::Action::SyncTarget(second_id, false)));
        assert!(!app.tools.sync.armed());
        assert!(app.tools.sync.destinations(first_id).is_empty());
    }

    #[test]
    fn forwarding_preflight_is_advisory_and_does_not_claim_platform_support() {
        assert!(forwarding_preflight_hint(None, None, false, false).is_empty());
        assert!(forwarding_preflight_hint(Some(false), Some(false), false, false).is_empty());
        let configured = forwarding_preflight_hint(Some(true), Some(true), true, true);
        if cfg!(windows) {
            assert!(configured.contains("agent forwarding is not native-runtime verified"));
        }
        if cfg!(windows) || cfg!(target_os = "macos") {
            assert!(configured.contains("X11 forwarding on this platform"));
        } else {
            assert!(configured.is_empty());
        }
        let no_display = forwarding_preflight_hint(None, Some(true), true, false);
        assert!(no_display.contains("DISPLAY is unset"));
        assert!(!no_display.contains("unsupported"));
        let no_agent = forwarding_preflight_hint(Some(true), None, false, true);
        if cfg!(unix) {
            assert!(no_agent.contains("SSH_AUTH_SOCK is unset"));
        } else {
            assert!(
                no_agent.contains("not native-runtime verified"),
                "Windows agent service capability must be explicit without using SSH_AUTH_SOCK"
            );
        }
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
        assert!(app.status.contains("Leave local navigation or free-type"));
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
        let plain = keyboard::Modifiers::default();
        assert_eq!(
            local_navigation_scroll(&keyboard::Key::Named(Named::ArrowUp), plain),
            Some(1)
        );
        assert_eq!(
            local_navigation_scroll(&keyboard::Key::Named(Named::PageDown), plain),
            Some(-16)
        );
        assert_eq!(
            local_navigation_scroll(&keyboard::Key::Character("j".into()), plain),
            Some(-1)
        );
        assert_eq!(
            local_navigation_scroll(&keyboard::Key::Character("k".into()), plain),
            Some(1)
        );
        assert_eq!(
            local_navigation_scroll(&keyboard::Key::Character("g".into()), plain),
            Some(32767)
        );
        assert_eq!(
            local_navigation_scroll(
                &keyboard::Key::Character("G".into()),
                keyboard::Modifiers::SHIFT
            ),
            Some(-32767)
        );
        assert_eq!(
            local_navigation_scroll(
                &keyboard::Key::Character("u".into()),
                keyboard::Modifiers::CTRL
            ),
            Some(16)
        );
        assert_eq!(
            local_navigation_scroll(
                &keyboard::Key::Character("d".into()),
                keyboard::Modifiers::CTRL
            ),
            Some(-16)
        );
        assert_eq!(
            local_navigation_scroll(
                &keyboard::Key::Character("j".into()),
                keyboard::Modifiers::ALT
            ),
            None
        );
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
    fn wheel_scroll_lines_rounds_and_ignores_zero() {
        assert_eq!(
            wheel_scroll_lines(&mouse::ScrollDelta::Lines { x: 0.0, y: -2.4 }, 16.0),
            Some(-2)
        );
        assert_eq!(
            wheel_scroll_lines(&mouse::ScrollDelta::Pixels { x: 0.0, y: 40.0 }, 16.0),
            Some(3)
        );
        assert_eq!(
            wheel_scroll_lines(&mouse::ScrollDelta::Pixels { x: 0.0, y: 2.0 }, 16.0),
            None
        );
    }

    #[test]
    fn terminal_canvas_ignores_wheel_events_outside_its_bounds() {
        use iced::widget::canvas::Program;
        let snapshot = terminal_core::DisplaySnapshot {
            rows: 24,
            columns: 80,
            background: [0, 0, 0],
            row_ranges: vec![(0, 0); 24],
            cells: Vec::new(),
        };
        let (mut panes, pane) = pane_grid::State::new(0_u8);
        let _ = &mut panes;
        let canvas = TerminalCanvas {
            focused: true,
            remote_key_mode: false,
            pane,
            id: 7,
            generation: 0,
            terminal_mode: terminal_core::TerminalMode::empty(),
            snapshot: &snapshot,
            appearance: crate::appearance::TerminalAppearance::default(),
        };
        let mut state = TerminalCanvasState::default();
        let bounds = iced::Rectangle::new(
            iced::Point::new(100.0, 100.0),
            iced::Size::new(200.0, 200.0),
        );
        let event = iced::Event::Mouse(mouse::Event::WheelScrolled {
            delta: mouse::ScrollDelta::Lines { x: 0.0, y: -3.0 },
        });
        // Prime last_position as a real session would after the pointer has
        // visited the pane; the wheel must still be ignored once it leaves.
        state.last_position = Some(iced::Point::new(50.0, 50.0));
        let outside = mouse::Cursor::Available(iced::Point::new(10.0, 10.0));
        assert!(canvas.update(&mut state, &event, bounds, outside).is_none());
        assert!(
            canvas
                .update(&mut state, &event, bounds, mouse::Cursor::Unavailable)
                .is_none()
        );
        let inside = mouse::Cursor::Available(iced::Point::new(150.0, 150.0));
        assert!(canvas.update(&mut state, &event, bounds, inside).is_some());
    }

    #[test]
    fn unix_mode_strings_match_ls() {
        assert_eq!(unix_mode_string('-', 0o644), "-rw-r--r--");
        assert_eq!(unix_mode_string('d', 0o755), "drwxr-xr-x");
        assert_eq!(unix_mode_string('-', 0o4755), "-rwsr-xr-x");
        assert_eq!(unix_mode_string('d', 0o1777), "drwxrwxrwt");
        assert_eq!(unix_mode_string('-', 0o2644), "-rw-r-Sr--");
    }

    #[test]
    fn file_kind_labels() {
        assert_eq!(windterm_type("src", true), "Directory");
        assert_eq!(windterm_type("Cargo.toml", false), "TOML File");
        assert_eq!(windterm_type("README", false), "File");
        assert_eq!(windterm_type(".bash_history", false), "BASH_HISTORY File");
        assert_eq!(windterm_size(1), "1 Byte");
        assert_eq!(windterm_size(18), "18 Bytes");
        assert_eq!(windterm_size(8192), "8.0 KB");
        assert_eq!(windterm_size(146_066_636), "139.3 MB");
        assert_eq!(windterm_size(5_476_083_302), "5.1 GB");
        let today = chrono::NaiveDate::from_ymd_opt(2026, 9, 10).unwrap();
        assert_eq!(
            windterm_remote_date("Sep 9 12:04", today),
            "2026/09/09 12:04"
        );
        assert_eq!(
            windterm_remote_date("Dec 1 08:00", today),
            "2025/12/01 08:00"
        );
        assert_eq!(windterm_remote_date("Mar 3 2024", today), "2024/03/03");
        assert_eq!(windterm_remote_date("garbage", today), "garbage");
    }

    #[test]
    fn local_path_input_expands_home_and_trims() {
        assert_eq!(expand_local_path("   "), None);
        assert_eq!(
            expand_local_path("  /tmp/x "),
            Some(std::path::PathBuf::from("/tmp/x"))
        );
        let home = directories::BaseDirs::new().map(|dirs| dirs.home_dir().to_path_buf());
        assert_eq!(expand_local_path("~"), home.clone());
        assert_eq!(
            expand_local_path("~/sub"),
            home.map(|home| home.join("sub"))
        );
    }

    #[test]
    fn local_path_submit_rejects_non_directories_and_navigates_to_directories() {
        let path = std::env::temp_dir().join(format!(
            "inspirum-iced-files-path-{}-nonexistent.json",
            std::process::id()
        ));
        let mut app = App::boot(path, None);
        app.files.local_path_input = "/definitely/not/a/dir".into();
        let _ = app.update(Message::FilesLocalPathSubmit);
        assert!(
            app.files
                .local_error
                .as_deref()
                .unwrap()
                .starts_with("Not a directory")
        );

        let temp = tempfile::tempdir().unwrap();
        app.files.local_path_input = format!("  {}  ", temp.path().display());
        let _ = app.update(Message::FilesLocalPathSubmit);
        assert_eq!(app.files.local_dir, temp.path());
        assert_eq!(app.files.local_path_input, temp.path().to_string_lossy());
        assert!(app.files.local_loading);
    }

    #[test]
    fn upload_conflict_check_does_not_need_to_clone_remote_directory() {
        let entries = vec![
            sftp::RemoteEntry::new("existing.txt", false, Some(10)),
            sftp::RemoteEntry::new("folder", true, None),
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

    fn pressed_key(key: keyboard::Key, modifiers: keyboard::Modifiers) -> Message {
        Message::Event(iced::Event::Keyboard(keyboard::Event::KeyPressed {
            modified_key: key.clone(),
            key,
            physical_key: keyboard::key::Physical::Code(keyboard::key::Code::Escape),
            location: keyboard::Location::Standard,
            modifiers,
            text: None,
            repeat: false,
        }))
    }

    #[test]
    fn menus_block_shortcuts_and_paste_without_dismissing_the_overlay() {
        for dock in [false, true] {
            let path = std::env::temp_dir().join(format!(
                "inspirum-menu-keys-{}-{dock}-nonexistent.json",
                std::process::id()
            ));
            let mut app = App::boot(path, None);
            if dock {
                app.dock_menu = Some(dock::Area::Bottom);
            } else {
                app.open_menu = Some(menu::Menu::Session);
            }
            let _ = app.update(pressed_key(
                keyboard::Key::Character("n".into()),
                keyboard::Modifiers::CTRL,
            ));
            assert!(
                app.dialog.is_none(),
                "menu must block application shortcuts"
            );
            let status = app.status.clone();
            let _ = app.update(pressed_key(
                keyboard::Key::Character("v".into()),
                keyboard::Modifiers::CTRL | keyboard::Modifiers::SHIFT,
            ));
            assert_eq!(app.status, status, "menu must block terminal paste");
            assert!(
                app.open_menu.is_some() || app.dock_menu.is_some(),
                "non-Escape keys must not dismiss the menu"
            );
            let _ = app.update(pressed_key(
                keyboard::Key::Named(keyboard::key::Named::Escape),
                keyboard::Modifiers::default(),
            ));
            assert!(app.open_menu.is_none() && app.dock_menu.is_none());
        }
    }

    #[test]
    fn menu_blocks_terminal_ime_cursor_updates() {
        let path = std::env::temp_dir().join(format!(
            "inspirum-menu-ime-{}-nonexistent.json",
            std::process::id()
        ));
        let mut app = App::boot(path, None);
        let profile = Session::default();
        let pane = app.new_terminal_pane(profile.clone());
        let id = pane.id;
        app.tabs.push(Workspace::new(profile, pane));
        app.open_menu = Some(menu::Menu::Edit);
        let _ = app.update(Message::TerminalImeCursor(id, iced::Rectangle::default()));
        assert!(
            app.ime_cursor.is_none(),
            "IME target must be disabled under a menu"
        );
    }

    #[test]
    fn ignored_escape_releases_sender_without_forwarding_or_erasing_draft() {
        let path = std::env::temp_dir().join(format!(
            "inspirum-sender-ignored-escape-{}-nonexistent.json",
            std::process::id()
        ));
        let mut app = App::boot(path, None);
        app.stage_command("keep this draft".into());
        app.sender_focused = true;
        let _ = app.update(pressed_key(
            keyboard::Key::Named(keyboard::key::Named::Escape),
            keyboard::Modifiers::default(),
        ));
        assert!(
            !app.sender_focused,
            "Escape must release Sender focus locally"
        );
        assert_eq!(app.command_sender, "keep this draft");
    }

    #[test]
    fn escape_releases_sender_focus_only_without_menu_or_dialog() {
        let path = std::env::temp_dir().join(format!(
            "inspirum-sender-escape-{}-nonexistent.json",
            std::process::id()
        ));
        let mut app = App::boot(path, None);
        app.sender_focused = true;
        let _ = app.update(Message::DismissModalKey);
        assert!(!app.sender_focused);
    }

    #[test]
    fn fresh_app_bottom_dock_has_sender_and_keeps_it_after_files_toggle() {
        let path = std::env::temp_dir().join(format!(
            "inspirum-sender-dock-{}-nonexistent.json",
            std::process::id()
        ));
        let mut app = App::boot(path, None);
        assert_eq!(
            app.docks.area(dock::Area::Bottom).panels,
            vec![
                dock::Panel::Sender,
                dock::Panel::Session,
                dock::Panel::Transfer,
                dock::Panel::Filer,
                dock::Panel::Explorer,
            ]
        );
        assert!(app.docks.is_visible(dock::Panel::Session));
        let profile = Session {
            host: "example.invalid".into(),
            name: "sender fixture".into(),
            ..Session::default()
        };
        let pane = app.new_terminal_pane(profile.clone());
        app.tabs.push(Workspace::new(profile, pane));
        app.toggle_files();
        app.toggle_files();
        app.toggle_files();
        assert!(app.docks.area_of(dock::Panel::Sender).is_some());
        assert!(app.docks.area_of(dock::Panel::Session).is_some());
        app.sender_focused = true;
        let _ = app.update(Message::DockActivate(dock::Panel::Sender));
        assert!(app.sender_focused);
        let _ = app.update(Message::DockActivate(dock::Panel::Session));
        assert!(!app.sender_focused);
        app.sender_focused = true;
        let _ = app.update(Message::SelectTab(0));
        assert!(!app.sender_focused);
    }

    #[test]
    fn dock_controls_persist_close_toggle_and_files_shortcut() {
        let profiles = std::env::temp_dir().join(format!(
            "inspirum-dock-controls-{}-nonexistent.json",
            std::process::id()
        ));
        let state_path = std::env::temp_dir().join(format!(
            "inspirum-dock-controls-{}-ui-state.json",
            std::process::id()
        ));
        let mut app = App::boot(profiles, None);
        app.ui_state_path = Some(state_path.clone());
        let _ = app.update(Message::DockMove(dock::Panel::Filer, dock::Area::Right));
        let _ = app.update(Message::DockClose(dock::Panel::Filer));
        assert_eq!(
            ui_state::load(&state_path)
                .docks
                .unwrap()
                .area_of(dock::Panel::Filer),
            None
        );
        let _ = app.update(Message::DockToggle(dock::Panel::Filer));
        assert_eq!(
            ui_state::load(&state_path)
                .docks
                .unwrap()
                .area_of(dock::Panel::Filer),
            Some(dock::Area::Right)
        );

        let profile = Session {
            host: "example.invalid".into(),
            ..Session::default()
        };
        let pane = app.new_terminal_pane(profile.clone());
        app.tabs.push(Workspace::new(profile, pane));
        let _ = app.update(Message::ToggleFiles);
        assert_eq!(ui_state::load(&state_path).docks.unwrap(), app.docks);
        let _ = app.update(Message::ToggleFiles);
        assert_eq!(ui_state::load(&state_path).docks.unwrap(), app.docks);
        std::fs::remove_file(state_path).unwrap();
    }

    #[test]
    fn files_explorer_can_move_to_side_without_losing_transfer_context() {
        let path = std::env::temp_dir().join(format!(
            "inspirum-files-dock-{}-nonexistent.json",
            std::process::id()
        ));
        let mut app = App::boot(path, None);
        let profile = Session {
            host: "example.invalid".into(),
            name: "dock fixture".into(),
            ..Session::default()
        };
        let pane = app.new_terminal_pane(profile.clone());
        app.tabs.push(Workspace::new(profile, pane));
        app.files.session_key = Some("preserve-transfer-target".into());
        app.files.remote_dir = "/work/important".into();
        app.toggle_files();
        assert!(app.files_open());
        assert_eq!(
            app.docks.area_of(dock::Panel::Filer),
            Some(dock::Area::Bottom)
        );
        assert!(
            app.docks.is_visible(dock::Panel::Filer),
            "Filer opens last, active"
        );
        assert!(app.docks.area_of(dock::Panel::Explorer).is_some());
        assert!(app.docks.area_of(dock::Panel::Transfer).is_some());
        app.docks.move_to(dock::Panel::Filer, dock::Area::Right);
        assert_eq!(
            app.docks.area_of(dock::Panel::Filer),
            Some(dock::Area::Right)
        );
        assert!(app.files_open());
        assert_eq!(app.files.remote_dir, "/work/important");
        assert_eq!(
            app.files.session_key.as_deref(),
            Some("preserve-transfer-target")
        );
        app.toggle_files();
        assert!(!app.files_open());
        for panel in FILES_PANELS {
            assert_eq!(app.docks.area_of(panel), None);
        }
    }

    #[test]
    fn auxiliary_shell_is_a_separate_ssh_pane_with_safe_teardown_and_reconnect() {
        let path = std::env::temp_dir().join(format!(
            "inspirum-aux-shell-{}-nonexistent.json",
            std::process::id()
        ));
        let mut app = App::boot(path, None);
        let profile = Session {
            name: "aux-shell fixture".into(),
            host: "example.invalid".into(),
            ..Session::default()
        };
        let original = app.new_terminal_pane(profile.clone());
        let original_id = original.id;
        app.tabs.push(Workspace::new(profile, original));

        let _ = app.update(Message::ToggleAuxiliaryShell);
        let tab = &app.tabs[0];
        assert_eq!(tab.panes.len(), 2);
        let (shell_id, shell_pane) = tab
            .panes
            .iter()
            .find(|(_, pane)| pane.auxiliary_shell)
            .expect("auxiliary SSH pane created");
        assert_ne!(shell_pane.id, original_id);
        assert!(!shell_pane.sftp, "auxiliary shell must be a PTY, not SFTP");
        let shell_id = *shell_id;
        let _ = app.update(Message::Reconnect(shell_id));
        assert!(
            app.tabs[0].panes.get(shell_id).unwrap().auxiliary_shell,
            "reconnect must keep the shell role"
        );

        let _ = app.update(Message::ToggleAuxiliaryShell);
        assert_eq!(app.tabs[0].panes.len(), 1);
        assert_eq!(
            app.tabs[0].panes.iter().next().unwrap().1.id,
            original_id,
            "closing auxiliary shell must preserve main terminal"
        );
        let _ = app.update(Message::ToggleAuxiliaryShell);
        assert_eq!(app.tabs[0].panes.len(), 2);
        let _ = app.update(Message::TogglePrivacyLock);
        let _ = app.update(Message::ToggleAuxiliaryShell);
        assert_eq!(
            app.tabs[0].panes.len(),
            2,
            "privacy lock blocks shell changes"
        );
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

    fn arm_synced_pair(label: &str) -> (App, u64, u64) {
        let path = std::env::temp_dir().join(format!(
            "inspirum-{label}-{}-nonexistent.json",
            std::process::id()
        ));
        let mut app = App::boot(path, None);
        let profile = Session {
            name: "fake-disconnected-sync".into(),
            host: "example.invalid".into(),
            ..Session::default()
        };
        let first = app.new_terminal_pane(profile.clone());
        let first_id = first.id;
        app.tabs.push(Workspace::new(profile.clone(), first));
        let second = app.new_terminal_pane(profile);
        let second_id = second.id;
        app.tabs[0].split(pane_grid::Axis::Vertical, second);
        let _ = app.update(Message::Tool(tools::Action::SyncTarget(first_id, true)));
        let _ = app.update(Message::Tool(tools::Action::SyncTarget(second_id, true)));
        let _ = app.update(Message::Tool(tools::Action::SyncArm(true)));
        assert!(app.tools.sync.armed(), "two selected panes must arm sync");
        let focused_id = app.tabs[0]
            .panes
            .get(app.tabs[0].focus)
            .map(|pane| pane.id)
            .expect("focused pane");
        assert!(
            focused_id == first_id || focused_id == second_id,
            "focused pane {focused_id} must be one of the synced panes"
        );
        (app, first_id, second_id)
    }

    /// `pressed_key` sets `text: None`. A plain character only becomes PTY
    /// bytes when the event carries committed text (`terminal_key_bytes`).
    fn pressed_char(ch: &str) -> Message {
        let key = keyboard::Key::Character(ch.into());
        Message::Event(iced::Event::Keyboard(keyboard::Event::KeyPressed {
            modified_key: key.clone(),
            key,
            physical_key: keyboard::key::Physical::Code(keyboard::key::Code::Escape),
            location: keyboard::Location::Standard,
            modifiers: keyboard::Modifiers::default(),
            text: Some(ch.into()),
            repeat: false,
        }))
    }

    #[test]
    fn enter_with_open_menu_writes_to_neither_source_nor_synced_pane() {
        let (mut app, first_id, second_id) = arm_synced_pair("menu-enter");
        let plain = keyboard::Modifiers::default();
        for (open_menu, dock_menu) in [
            (Some(menu::Menu::Session), None),
            (None, Some(dock::Area::Bottom)),
        ] {
            app.open_menu = open_menu;
            app.dock_menu = dock_menu;
            app.pty_write_log.clear();
            let _ = app.update(pressed_key(
                keyboard::Key::Named(keyboard::key::Named::Enter),
                plain,
            ));
            let _ = app.update(pressed_key(keyboard::Key::Character("x".into()), plain));
            let _ = app.update(pressed_char("x"));
            assert!(
                app.pty_write_log.is_empty(),
                "open menu must write to neither source nor synced pane, log={:?}",
                app.pty_write_log
            );
            assert!(
                app.open_menu.is_some() || app.dock_menu.is_some(),
                "non-Escape keys must leave the menu open"
            );
            app.open_menu = None;
            app.dock_menu = None;
            app.pty_write_log.clear();
            let _ = app.update(pressed_key(
                keyboard::Key::Named(keyboard::key::Named::Enter),
                plain,
            ));
            assert!(
                app.pty_write_log.iter().any(|(id, _)| *id == first_id)
                    && app.pty_write_log.iter().any(|(id, _)| *id == second_id),
                "recorder must see Enter on both synced panes, log={:?}",
                app.pty_write_log
            );
        }
    }

    #[test]
    fn escape_in_sender_writes_nothing_and_releases_focus() {
        let (mut app, _first_id, _second_id) = arm_synced_pair("sender-escape");
        assert!(app.open_menu.is_none() && app.dock_menu.is_none());
        app.sender_focused = true;
        app.pty_write_log.clear();
        let _ = app.update(pressed_key(
            keyboard::Key::Named(keyboard::key::Named::Escape),
            keyboard::Modifiers::default(),
        ));
        assert!(
            app.pty_write_log.is_empty(),
            "Sender Escape must not reach any pane, log={:?}",
            app.pty_write_log
        );
        assert!(
            !app.sender_focused,
            "Escape must release Sender focus locally"
        );
        app.pty_write_log.clear();
        let _ = app.update(pressed_key(
            keyboard::Key::Named(keyboard::key::Named::Escape),
            keyboard::Modifiers::default(),
        ));
        assert!(
            app.pty_write_log
                .iter()
                .any(|(_, bytes)| bytes.contains(&0x1b)),
            "Escape after releasing Sender must be logged as 0x1b, log={:?}",
            app.pty_write_log
        );
    }

    #[test]
    fn ime_commit_and_paste_write_nothing_while_menu_open() {
        let path = std::env::temp_dir().join(format!(
            "inspirum-menu-ime-paste-{}-nonexistent.json",
            std::process::id()
        ));
        let mut app = App::boot(path, None);
        let profile = Session::default();
        let pane = app.new_terminal_pane(profile.clone());
        let id = pane.id;
        app.tabs.push(Workspace::new(profile, pane));
        app.open_menu = Some(menu::Menu::Session);
        app.pty_write_log.clear();
        let _ = app.update(Message::TerminalImeCommit(id, "x".into()));
        let _ = app.update(Message::ClipboardRead(id, vec![id], Some("x".into())));
        assert!(
            app.pty_write_log.is_empty(),
            "menu must drop IME commit and paste, log={:?}",
            app.pty_write_log
        );
        assert!(app.open_menu.is_some(), "menu must stay open");
        app.open_menu = None;
        app.pty_write_log.clear();
        let _ = app.update(Message::TerminalImeCommit(id, "x".into()));
        assert!(
            app.pty_write_log
                .iter()
                .any(|(pane, bytes)| *pane == id && bytes.as_slice() == b"x"),
            "IME commit must be logged once the menu is closed, log={:?}",
            app.pty_write_log
        );
    }

    fn app_with_one_pane_and_auxiliary_shell(label: &str) -> App {
        let path = std::env::temp_dir().join(format!(
            "inspirum-{label}-{}-nonexistent.json",
            std::process::id()
        ));
        let mut app = App::boot(path, None);
        let profile = Session {
            name: "chord fixture".into(),
            host: "example.invalid".into(),
            ..Session::default()
        };
        let pane = app.new_terminal_pane(profile.clone());
        app.tabs.push(Workspace::new(profile, pane));
        let _ = app.update(Message::ToggleAuxiliaryShell);
        let tab = &app.tabs[0];
        assert_eq!(tab.panes.len(), 2);
        assert!(
            tab.panes.iter().any(|(_, pane)| pane.auxiliary_shell),
            "auxiliary SSH pane created"
        );
        app
    }

    fn every_key_chord() -> [KeyChord; 8] {
        [
            KeyChord::FocusMode,
            KeyChord::FreeType,
            KeyChord::RemoteKeys,
            KeyChord::LocalNavigation,
            KeyChord::SplitRight,
            KeyChord::ToggleFiles,
            KeyChord::AuxiliaryShell,
            KeyChord::Find,
        ]
    }

    #[test]
    fn key_chords_do_nothing_while_a_menu_is_open() {
        let mut app = app_with_one_pane_and_auxiliary_shell("chord-menu-blocks");
        let focus_mode = app.focus_mode;
        let files_open = app.files_open();
        for (open_menu, dock_menu) in [
            (Some(menu::Menu::Session), None),
            (None, Some(dock::Area::Bottom)),
        ] {
            app.open_menu = open_menu;
            app.dock_menu = dock_menu;
            for chord in every_key_chord() {
                let _ = app.update(Message::KeyChord(chord));
                assert_eq!(
                    app.tabs[0].panes.len(),
                    2,
                    "{chord:?} changed pane count while a menu was open"
                );
                assert!(
                    app.tabs[0]
                        .panes
                        .iter()
                        .any(|(_, pane)| pane.auxiliary_shell),
                    "{chord:?} removed the auxiliary shell while a menu was open"
                );
                assert_eq!(
                    app.focus_mode, focus_mode,
                    "{chord:?} changed focus mode while a menu was open"
                );
                assert!(
                    app.free_type.is_empty(),
                    "{chord:?} opened free type while a menu was open"
                );
                assert!(
                    app.remote_keys.is_empty(),
                    "{chord:?} enabled remote keys while a menu was open"
                );
                assert!(
                    app.local_navigation.is_empty(),
                    "{chord:?} enabled local navigation while a menu was open"
                );
                assert_eq!(
                    app.files_open(),
                    files_open,
                    "{chord:?} changed the files dock while a menu was open"
                );
                assert!(
                    app.open_menu.is_some() || app.dock_menu.is_some(),
                    "{chord:?} closed the menu"
                );
            }
        }
    }

    #[test]
    fn key_chords_work_again_after_the_menu_closes() {
        let mut app = app_with_one_pane_and_auxiliary_shell("chord-menu-closed");
        app.open_menu = Some(menu::Menu::Session);
        app.dock_menu = Some(dock::Area::Bottom);
        app.open_menu = None;
        app.dock_menu = None;
        assert!(!app.focus_mode);
        let _ = app.update(Message::KeyChord(KeyChord::AuxiliaryShell));
        assert_eq!(app.tabs[0].panes.len(), 1);
        assert!(
            !app.tabs[0]
                .panes
                .iter()
                .any(|(_, pane)| pane.auxiliary_shell),
            "auxiliary shell chord must close the shell once the menu is gone"
        );
        let _ = app.update(Message::KeyChord(KeyChord::FocusMode));
        assert!(
            app.focus_mode,
            "focus-mode chord must toggle once the menu is gone"
        );
    }

    #[test]
    fn privacy_lock_still_toggles_while_a_menu_is_open() {
        let mut app = app_with_one_pane_and_auxiliary_shell("privacy-lock-menu");
        app.open_menu = Some(menu::Menu::Session);
        assert!(!app.privacy_locked);
        let _ = app.update(Message::TogglePrivacyLock);
        assert!(app.privacy_locked);
        assert_eq!(app.open_menu, Some(menu::Menu::Session));
    }

    fn sort_reference() -> chrono::NaiveDate {
        chrono::NaiveDate::from_ymd_opt(2026, 11, 21).unwrap()
    }

    fn listing_entry(
        name: &str,
        is_dir: bool,
        size: Option<u64>,
        modified: Option<std::time::SystemTime>,
    ) -> LocalFileEntry {
        LocalFileEntry {
            path: std::path::PathBuf::from(name),
            name: name.to_owned(),
            is_dir,
            size,
            permissions: "-rw-r--r--".into(),
            owner: "owner".into(),
            group: "group".into(),
            modified,
        }
    }

    fn listing_names(entries: &[LocalFileEntry]) -> Vec<&str> {
        entries.iter().map(|entry| entry.name.as_str()).collect()
    }

    #[test]
    fn file_sort_default_is_directories_then_name_ascending() {
        let mut entries = vec![
            listing_entry("b", false, Some(3), None),
            listing_entry("a", true, None, None),
            listing_entry("A", false, Some(1), None),
            listing_entry("B", true, None, None),
        ];
        sort_file_entries(&mut entries, FileSortColumn::Name, true, sort_reference());
        assert_eq!(listing_names(&entries), ["a", "B", "A", "b"]);
        assert!(entries.iter().take(2).all(|entry| entry.is_dir));
        assert!(entries.iter().skip(2).all(|entry| !entry.is_dir));
    }

    #[test]
    fn file_sort_size_descending_keeps_directories_first() {
        let mut entries = vec![
            listing_entry("small-dir", true, Some(1), None),
            listing_entry("huge", false, Some(100), None),
            listing_entry("big-dir", true, Some(50), None),
            listing_entry("tiny", false, Some(2), None),
        ];
        sort_file_entries(&mut entries, FileSortColumn::Size, false, sort_reference());
        assert_eq!(
            listing_names(&entries),
            ["big-dir", "small-dir", "huge", "tiny"]
        );
        assert!(entries.iter().take(2).all(|entry| entry.is_dir));
        assert!(entries.iter().skip(2).all(|entry| !entry.is_dir));
    }

    #[test]
    fn clicking_the_same_column_twice_reverses_sort() {
        let once = next_file_sort(FileSortColumn::Name, true, FileSortColumn::Size);
        assert_eq!(once, (FileSortColumn::Size, true));
        let twice = next_file_sort(once.0, once.1, FileSortColumn::Size);
        assert_eq!(twice, (FileSortColumn::Size, false));
        let third = next_file_sort(twice.0, twice.1, FileSortColumn::Size);
        assert_eq!(third, (FileSortColumn::Size, true));

        let mut ascending = vec![
            listing_entry("c", false, Some(3), None),
            listing_entry("a", false, Some(1), None),
            listing_entry("b", false, Some(2), None),
        ];
        let mut descending = ascending.clone();
        sort_file_entries(&mut ascending, once.0, once.1, sort_reference());
        sort_file_entries(&mut descending, twice.0, twice.1, sort_reference());
        assert_eq!(listing_names(&ascending), ["a", "b", "c"]);
        assert_eq!(listing_names(&descending), ["c", "b", "a"]);
    }

    #[test]
    fn file_sort_breaks_size_ties_by_lowercase_name() {
        let mut entries = vec![
            listing_entry("b.txt", false, Some(5), None),
            listing_entry("A.txt", false, Some(5), None),
            listing_entry("c.txt", false, Some(9), None),
        ];
        sort_file_entries(&mut entries, FileSortColumn::Size, true, sort_reference());
        assert_eq!(listing_names(&entries), ["A.txt", "b.txt", "c.txt"]);

        // Descending size still breaks the equal-size tie by lowercase name,
        // not by reversed name.
        sort_file_entries(&mut entries, FileSortColumn::Size, false, sort_reference());
        assert_eq!(listing_names(&entries), ["c.txt", "A.txt", "b.txt"]);
    }

    #[test]
    fn file_sort_dates_use_stored_timestamp_or_chronological_ls_date() {
        let earlier = std::time::SystemTime::UNIX_EPOCH;
        let later = earlier + std::time::Duration::from_secs(60);
        let mut local = vec![
            listing_entry("later", false, Some(1), Some(later)),
            listing_entry("earlier", false, Some(1), Some(earlier)),
        ];
        sort_file_entries(
            &mut local,
            FileSortColumn::DateModified,
            true,
            sort_reference(),
        );
        assert_eq!(listing_names(&local), ["earlier", "later"]);

        // "Jan" < "Oct" as text, but January 2024 is after October 2023.
        let mut remote = vec![
            {
                let mut entry = sftp::RemoteEntry::new("jan", false, Some(1));
                entry.modified = Some("Jan 2 2024".into());
                entry
            },
            {
                let mut entry = sftp::RemoteEntry::new("oct", false, Some(1));
                entry.modified = Some("Oct 18 2023".into());
                entry
            },
        ];
        sort_file_entries(
            &mut remote,
            FileSortColumn::DateModified,
            true,
            sort_reference(),
        );
        assert_eq!(
            remote
                .iter()
                .map(|entry| entry.name.as_str())
                .collect::<Vec<_>>(),
            ["oct", "jan"]
        );
    }

    #[test]
    fn ls_modified_sort_key_december_is_previous_year_in_mid_january() {
        let now = chrono::NaiveDate::from_ymd_opt(2026, 1, 15).unwrap();
        let december = ls_modified_sort_key("Dec 30 10:00", now).unwrap();
        let january = ls_modified_sort_key("Jan 02 09:00", now).unwrap();
        assert_eq!(december, (2025, 12, 30, 10, 0));
        assert_eq!(january, (2026, 1, 2, 9, 0));
        assert!(december < january);
    }

    #[test]
    fn ls_modified_sort_key_year_form_sorts_before_recent_time_form() {
        let now = chrono::NaiveDate::from_ymd_opt(2026, 11, 21).unwrap();
        let march = ls_modified_sort_key("Mar 3 2024", now).unwrap();
        let october = ls_modified_sort_key("Oct 18 09:41", now).unwrap();
        assert_eq!(march, (2024, 3, 3, 0, 0));
        assert_eq!(october, (2026, 10, 18, 9, 41));
        assert!(march < october);
    }

    #[test]
    fn ls_modified_sort_key_accepts_single_and_double_digit_days() {
        let now = chrono::NaiveDate::from_ymd_opt(2026, 11, 21).unwrap();
        assert_eq!(
            ls_modified_sort_key("Mar 3 2024", now),
            Some((2024, 3, 3, 0, 0))
        );
        assert_eq!(
            ls_modified_sort_key("Mar 03 2024", now),
            Some((2024, 3, 3, 0, 0))
        );
        // "Oct 18" < "Oct 8" as text; the day numbers sort the other way.
        let single = ls_modified_sort_key("Oct 8 09:41", now);
        let double = ls_modified_sort_key("Oct 18 09:41", now);
        assert_eq!(single, Some((2026, 10, 8, 9, 41)));
        assert_eq!(double, Some((2026, 10, 18, 9, 41)));
        assert!(single < double);
    }

    #[test]
    fn ls_modified_sort_key_garbage_sorts_before_parsed_dates() {
        let now = chrono::NaiveDate::from_ymd_opt(2026, 11, 21).unwrap();
        assert_eq!(ls_modified_sort_key("not-a-date", now), None);
        assert_eq!(ls_modified_sort_key("", now), None);

        let mut remote = vec![
            {
                let mut entry = sftp::RemoteEntry::new("zeta", false, Some(1));
                entry.modified = Some("Mar 3 2024".into());
                entry
            },
            sftp::RemoteEntry::new("missing", false, Some(1)),
            {
                let mut entry = sftp::RemoteEntry::new("garbage", false, Some(1));
                entry.modified = Some("not-a-date".into());
                entry
            },
        ];
        sort_file_entries(
            &mut remote,
            FileSortColumn::DateModified,
            true,
            sort_reference(),
        );
        assert_eq!(
            remote
                .iter()
                .map(|entry| entry.name.as_str())
                .collect::<Vec<_>>(),
            ["garbage", "missing", "zeta"]
        );
    }

    #[test]
    fn file_column_click_reverses_only_that_side() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("profiles.json");
        std::fs::write(&path, b"[]").unwrap();
        let mut app = App::boot(path, None);
        assert_eq!(app.local_file_sort, FileSortColumn::Name);
        assert!(app.local_file_sort_ascending);
        assert_eq!(app.remote_file_sort, FileSortColumn::Name);
        assert!(app.remote_file_sort_ascending);

        let _ = app.update(Message::FilesSortLocal(FileSortColumn::Size));
        assert_eq!(
            (app.local_file_sort, app.local_file_sort_ascending),
            (FileSortColumn::Size, true)
        );
        assert_eq!(
            (app.remote_file_sort, app.remote_file_sort_ascending),
            (FileSortColumn::Name, true)
        );

        let _ = app.update(Message::FilesSortLocal(FileSortColumn::Size));
        assert!(!app.local_file_sort_ascending);
        assert!(app.remote_file_sort_ascending);

        let _ = app.update(Message::FilesSortRemote(FileSortColumn::Owner));
        assert_eq!(app.remote_file_sort, FileSortColumn::Owner);
        assert!(app.remote_file_sort_ascending);
        assert_eq!(app.local_file_sort, FileSortColumn::Size);
        assert!(!app.local_file_sort_ascending);
    }

    fn push_open_session(app: &mut App, name: &str) {
        let profile = Session {
            name: name.into(),
            host: "example.invalid".into(),
            ..Session::default()
        };
        let pane = app.new_terminal_pane(profile.clone());
        app.tabs.push(Workspace::new(profile, pane));
    }

    #[test]
    fn exit_with_open_tab_opens_confirmation_and_keeps_tabs() {
        let path = std::env::temp_dir().join(format!(
            "inspirum-exit-open-{}-nonexistent.json",
            std::process::id()
        ));
        let mut app = App::boot(path, None);
        push_open_session(&mut app, "exit-open");
        let _ = app.update(Message::MenuCommand(menu::Command::Exit));
        assert!(matches!(app.dialog, Some(Dialog::Exit)));
        assert_eq!(app.tabs.len(), 1);
        assert_eq!(
            exit_confirm_body(1),
            "1 session is open. Exit and disconnect it?"
        );
        assert_eq!(
            exit_confirm_body(2),
            "2 sessions are open. Exit and disconnect them?"
        );
    }

    #[test]
    fn exit_confirmation_text_singular_for_one_and_plural_otherwise() {
        assert_eq!(
            exit_confirm_body(1),
            "1 session is open. Exit and disconnect it?"
        );
        assert_eq!(
            exit_confirm_body(3),
            "3 sessions are open. Exit and disconnect them?"
        );
    }

    #[test]
    fn exit_cancel_leaves_tabs_intact_and_dialog_closed() {
        let path = std::env::temp_dir().join(format!(
            "inspirum-exit-cancel-{}-nonexistent.json",
            std::process::id()
        ));
        let mut app = App::boot(path, None);
        push_open_session(&mut app, "exit-cancel");
        let _ = app.update(Message::Exit);
        assert!(matches!(app.dialog, Some(Dialog::Exit)));
        let _ = app.update(Message::CloseDialog);
        assert!(app.dialog.is_none());
        assert_eq!(app.tabs.len(), 1);
        let _ = app.update(Message::Exit);
        let _ = app.update(pressed_key(
            keyboard::Key::Named(keyboard::key::Named::Escape),
            keyboard::Modifiers::default(),
        ));
        assert!(app.dialog.is_none());
        assert_eq!(app.tabs.len(), 1);
    }

    #[test]
    fn exit_with_no_tabs_opens_no_dialog() {
        let path = std::env::temp_dir().join(format!(
            "inspirum-exit-empty-{}-nonexistent.json",
            std::process::id()
        ));
        let mut app = App::boot(path, None);
        assert!(app.tabs.is_empty());
        // Close task is iced::exit(); this test asserts state only.
        let _ = app.update(Message::Exit);
        assert!(app.dialog.is_none());
        assert!(app.tabs.is_empty());
        let _ = app.update(Message::MenuCommand(menu::Command::Exit));
        assert!(app.dialog.is_none());
        assert!(app.tabs.is_empty());
    }

    #[test]
    fn menu_exit_does_not_bypass_privacy_lock_or_modal() {
        let path = std::env::temp_dir().join(format!(
            "inspirum-exit-gated-{}-nonexistent.json",
            std::process::id()
        ));
        let mut app = App::boot(path, None);
        push_open_session(&mut app, "exit-gated");
        app.privacy_locked = true;
        app.open_menu = Some(menu::Menu::Session);
        let _ = app.update(Message::MenuCommand(menu::Command::Exit));
        assert!(app.dialog.is_none());
        assert!(app.privacy_locked);
        assert_eq!(app.tabs.len(), 1);
        assert_eq!(app.open_menu, Some(menu::Menu::Session));

        app.privacy_locked = false;
        app.dialog = Some(Dialog::About);
        let _ = app.update(Message::MenuCommand(menu::Command::Exit));
        assert!(matches!(app.dialog, Some(Dialog::About)));
        assert_eq!(app.tabs.len(), 1);
    }

    fn boot_sort_app(name: &str) -> (tempfile::TempDir, App) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(format!("{name}.json"));
        std::fs::write(&path, b"[]").unwrap();
        (dir, App::boot(path, None))
    }

    #[test]
    fn view_and_unrelated_updates_do_not_resort_file_lists() {
        let (_dir, mut app) = boot_sort_app("sort-cache-view");
        push_open_session(&mut app, "sort-cache");
        let pane_id = app.tabs[0].panes.iter().next().unwrap().1.id;

        let _ = app.update(Message::FilesLocalLoaded(
            0,
            Ok(vec![
                listing_entry("b", false, Some(2), None),
                listing_entry("a", true, None, None),
            ]),
        ));
        let mut january = sftp::RemoteEntry::new("january", false, Some(1));
        january.modified = Some("Jan 02 09:00".into());
        let mut december = sftp::RemoteEntry::new("december", false, Some(1));
        december.modified = Some("Dec 30 10:00".into());
        let _ = app.update(Message::FilesRemoteLoaded(0, Ok(vec![january, december])));

        let local_runs = app.files.local_sort_runs;
        let remote_runs = app.files.remote_sort_runs;
        let local_order = app.files.local_order.clone();
        let remote_order = app.files.remote_order.clone();

        let _ = app.view();
        let _ = app.file_lists(800.0);
        let _ = app.view();
        let _ = app.file_lists(800.0);
        let _ = app.file_lists(1200.0);
        let _ = app.update(Message::TerminalFrame);
        let _ = app.update(Message::PtyEvent(pane_id, terminal_core::PtyEvent::Wakeup));
        let _ = app.update(Message::PtyEvent(pane_id, terminal_core::PtyEvent::Bell));
        let _ = app.update(Message::TerminalFrame);
        let _ = app.view();
        let _ = app.file_lists(800.0);

        assert_eq!(app.files.local_sort_runs, local_runs);
        assert_eq!(app.files.remote_sort_runs, remote_runs);
        assert_eq!(app.files.local_order, local_order);
        assert_eq!(app.files.remote_order, remote_order);
    }

    #[test]
    fn sort_click_resorts_only_that_side_once() {
        let (_dir, mut app) = boot_sort_app("sort-cache-click");
        let _ = app.update(Message::FilesLocalLoaded(
            0,
            Ok(vec![
                listing_entry("local-b", false, Some(2), None),
                listing_entry("local-a", false, Some(8), None),
            ]),
        ));
        let _ = app.update(Message::FilesRemoteLoaded(
            0,
            Ok(vec![
                sftp::RemoteEntry::new("small", false, Some(1)),
                sftp::RemoteEntry::new("large", false, Some(9)),
                sftp::RemoteEntry::new("mid", false, Some(4)),
            ]),
        ));
        let local_runs = app.files.local_sort_runs;
        let remote_runs = app.files.remote_sort_runs;

        let _ = app.update(Message::FilesSortRemote(FileSortColumn::Size));

        assert_eq!(app.files.remote_sort_runs, remote_runs + 1);
        assert_eq!(app.files.local_sort_runs, local_runs);
        assert_eq!(app.files.remote_order.len(), app.files.remote_entries.len());

        let cached: Vec<String> = app
            .files
            .remote_order
            .iter()
            .map(|&index| app.files.remote_entries[index].name.clone())
            .collect();
        let mut sorted = app.files.remote_entries.clone();
        sort_file_entries(
            &mut sorted,
            app.remote_file_sort,
            app.remote_file_sort_ascending,
            app.files.remote_sort_reference,
        );
        let expected: Vec<String> = sorted.into_iter().map(|entry| entry.name).collect();
        assert_eq!(cached, expected);
    }

    #[test]
    fn new_listing_resorts_once_and_order_matches_entry_count() {
        let (_dir, mut app) = boot_sort_app("sort-cache-listing");
        let _ = app.update(Message::FilesLocalLoaded(
            0,
            Ok(vec![
                listing_entry("a", false, Some(1), None),
                listing_entry("b", true, None, None),
            ]),
        ));
        assert_eq!(app.files.local_sort_runs, 1);
        assert_eq!(app.files.local_order.len(), app.files.local_entries.len());

        let _ = app.update(Message::FilesRemoteLoaded(
            0,
            Ok(vec![
                sftp::RemoteEntry::new("r", false, Some(1)),
                sftp::RemoteEntry::new("d", true, None),
            ]),
        ));
        assert_eq!(app.files.remote_sort_runs, 1);
        assert_eq!(app.files.remote_order.len(), app.files.remote_entries.len());

        let _ = app.update(Message::FilesLocalLoaded(0, Err("gone".into())));
        assert_eq!(app.files.local_sort_runs, 2);
        assert!(app.files.local_entries.is_empty());
        assert_eq!(app.files.local_order.len(), app.files.local_entries.len());

        let _ = app.update(Message::FilesRemoteLoaded(0, Err("gone".into())));
        assert_eq!(app.files.remote_sort_runs, 2);
        assert!(app.files.remote_entries.is_empty());
        assert_eq!(app.files.remote_order.len(), app.files.remote_entries.len());
    }

    #[test]
    fn remote_date_sort_uses_stored_reference_date() {
        let (_dir, mut app) = boot_sort_app("sort-cache-reference");
        let mut january = sftp::RemoteEntry::new("january", false, Some(1));
        january.modified = Some("Jan 02 09:00".into());
        let mut december = sftp::RemoteEntry::new("december", false, Some(1));
        december.modified = Some("Dec 30 10:00".into());
        app.files.remote_entries = vec![january, december];
        app.files.remote_sort_reference = chrono::NaiveDate::from_ymd_opt(2026, 1, 15).unwrap();
        app.remote_file_sort = FileSortColumn::DateModified;
        app.remote_file_sort_ascending = true;
        app.resort_remote_files();

        let names: Vec<&str> = app
            .files
            .remote_order
            .iter()
            .map(|&index| app.files.remote_entries[index].name.as_str())
            .collect();
        assert_eq!(names, ["december", "january"]);
    }

    #[test]
    fn remote_date_label_and_sort_key_share_the_listing_reference() {
        let (_dir, mut app) = boot_sort_app("date-label-reference");
        let mut entry = sftp::RemoteEntry::new("rollover", false, Some(1));
        entry.modified = Some("Nov 22 00:01".into());

        let before_midnight = chrono::NaiveDate::from_ymd_opt(2026, 11, 21).unwrap();
        app.files.remote_sort_reference = before_midnight;
        let label = app.files.remote_modified_label(&entry);
        let again = app.files.remote_modified_label(&entry);
        let key_year = ls_modified_sort_key("Nov 22 00:01", before_midnight)
            .unwrap()
            .0;
        assert_eq!(label, "2025/11/22 00:01");
        assert_eq!(label, again);
        assert_eq!(key_year, 2025);
        assert!(label.starts_with(&format!("{key_year:04}/")));

        let after_midnight = chrono::NaiveDate::from_ymd_opt(2026, 11, 22).unwrap();
        app.files.remote_sort_reference = after_midnight;
        let label = app.files.remote_modified_label(&entry);
        let key_year = ls_modified_sort_key("Nov 22 00:01", after_midnight)
            .unwrap()
            .0;
        assert_eq!(label, "2026/11/22 00:01");
        assert_eq!(key_year, 2026);
        assert!(label.starts_with(&format!("{key_year:04}/")));
    }

    fn session_click_app() -> App {
        let path = std::env::temp_dir().join(format!(
            "inspirum-iced-session-click-{}-nonexistent.json",
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
        app
    }

    #[test]
    fn session_single_click_selects_and_opens_no_tab() {
        let mut app = session_click_app();
        let _ = app.update(Message::SelectSession(0));
        assert_eq!(app.selected_profile, Some(0));
        assert_eq!(
            app.selected_profile_key.as_deref(),
            Some(session_profile_key(&app.profiles[0]).as_str())
        );
        assert!(app.tabs.is_empty());

        // A click on a different row selects it and still opens nothing.
        let _ = app.update(Message::SelectSession(1));
        assert_eq!(app.selected_profile, Some(1));
        assert!(app.tabs.is_empty());
    }

    #[test]
    fn session_second_click_within_window_opens_exactly_one_tab() {
        let mut app = session_click_app();
        let _ = app.update(Message::SelectSession(1));
        assert!(app.tabs.is_empty());

        let _ = app.update(Message::SelectSession(1));
        assert_eq!(app.tabs.len(), 1);
        assert_eq!(app.selected_profile, Some(1));
        // The opening click consumes the gesture.
        assert!(app.session_click.is_none());

        let _ = app.update(Message::SelectSession(1));
        assert_eq!(
            app.tabs.len(),
            1,
            "a follow-up click must not open a second tab"
        );
    }

    fn remote_context_labels(is_dir: bool) -> Vec<&'static str> {
        file_context_entries(&FileContextMenu {
            side: FileContextSide::Remote,
            is_dir,
            open_path: "/srv/item".into(),
            local_path: None,
            anchor: iced::Point::ORIGIN,
        })
        .into_iter()
        .map(|(label, _)| label)
        .collect()
    }

    #[test]
    fn remote_context_menu_selects_the_row_and_escape_closes_it() {
        let mut app = app_with_one_pane_and_auxiliary_shell("filer-context-select");
        let remote_dir = app.files.remote_dir.clone();
        let file = sftp::RemoteEntry::new("notes.txt", false, Some(12));
        app.files.remote_entries = vec![file.clone()];
        let _ = app.update(Message::PointerMoved(iced::Point::new(48.0, 64.0)));
        let _ = app.update(Message::OpenRemoteFileContext {
            entry: file.clone(),
            open_path: "/srv/notes.txt".into(),
        });
        assert!(app.context_menu.is_some(), "right-click opens the menu");
        assert_eq!(
            app.context_menu.as_ref().map(ContextMenu::anchor),
            Some(iced::Point::new(48.0, 64.0))
        );
        assert_eq!(
            app.files
                .selected_remote
                .as_ref()
                .map(|entry| entry.name.as_str()),
            Some("notes.txt")
        );
        assert_eq!(
            remote_context_labels(false),
            vec![
                "Download",
                "Edit",
                "New Folder",
                "Rename",
                "Delete",
                "Refresh",
            ]
        );
        let _ = app.update(pressed_key(
            keyboard::Key::Named(keyboard::key::Named::Escape),
            keyboard::Modifiers::default(),
        ));
        assert!(app.context_menu.is_none(), "Escape closes the context menu");
        assert!(
            app.pty_write_log.is_empty(),
            "Escape must not reach the terminal"
        );

        let dir = sftp::RemoteEntry::new("src", true, None);
        let _ = app.update(Message::OpenRemoteFileContext {
            entry: dir,
            open_path: "/srv/src".into(),
        });
        assert_eq!(
            app.files.remote_dir, remote_dir,
            "right-click must not navigate"
        );
        assert!(
            app.files
                .selected_remote
                .as_ref()
                .is_some_and(|entry| entry.name == "src" && entry.is_dir)
        );
        assert_eq!(
            remote_context_labels(true),
            vec!["Open", "New Folder", "Rename", "Delete", "Refresh"]
        );
        let open = file_context_action_message(
            app.context_menu
                .as_ref()
                .and_then(ContextMenu::file)
                .expect("menu"),
            FileContextAction::Open,
        );
        assert!(matches!(open, Some(Message::FilesOpenRemote(path)) if path == "/srv/src"));
    }

    #[test]
    fn remote_context_menu_blocks_terminal_keys_and_chords() {
        let (mut app, first_id, _second_id) = arm_synced_pair("filer-context-keys");
        let focus_mode = app.focus_mode;
        let _ = app.update(Message::OpenRemoteFileContext {
            entry: sftp::RemoteEntry::new("notes.txt", false, Some(4)),
            open_path: "/srv/notes.txt".into(),
        });
        assert!(app.context_menu.is_some());
        app.pty_write_log.clear();
        let status = app.status.clone();
        let _ = app.update(pressed_char("x"));
        let _ = app.update(pressed_key(
            keyboard::Key::Character("x".into()),
            keyboard::Modifiers::default(),
        ));
        let _ = app.update(Message::KeyChord(KeyChord::FocusMode));
        let _ = app.update(Message::TerminalImeCursor(
            first_id,
            iced::Rectangle::default(),
        ));
        let _ = app.update(Message::TerminalImeCommit(first_id, "x".into()));
        let _ = app.update(Message::RequestPaste);
        assert!(
            app.pty_write_log.is_empty(),
            "open context menu must not write to a pane, log={:?}",
            app.pty_write_log
        );
        assert_eq!(app.focus_mode, focus_mode, "chords must not run");
        assert_eq!(app.status, status, "paste must not start");
        assert!(
            app.context_menu.is_some(),
            "non-Escape keys leave the menu open"
        );
        assert!(app.ime_cursor.is_none(), "IME target must stay disabled");

        app.context_menu = None;
        app.pty_write_log.clear();
        let _ = app.update(pressed_key(
            keyboard::Key::Named(keyboard::key::Named::Enter),
            keyboard::Modifiers::default(),
        ));
        assert!(
            app.pty_write_log.iter().any(|(id, _)| *id == first_id),
            "recorder must see Enter once the menu is gone, log={:?}",
            app.pty_write_log
        );
    }

    #[test]
    fn session_click_outside_window_does_not_open() {
        let mut app = session_click_app();
        let _ = app.update(Message::SelectSession(0));
        let key = session_profile_key(&app.profiles[0]);
        app.session_click = Instant::now()
            .checked_sub(Duration::from_millis(401))
            .map(|at| (at, key));
        let _ = app.update(Message::SelectSession(0));
        assert!(app.tabs.is_empty());
        assert_eq!(app.selected_profile, Some(0));
    }

    #[test]
    fn session_double_click_window_is_four_hundred_milliseconds() {
        let start = Instant::now();
        assert!(session_double_click(start, start));
        assert!(session_double_click(
            start,
            start + Duration::from_millis(400)
        ));
        assert!(!session_double_click(
            start,
            start + Duration::from_millis(401)
        ));
    }

    #[test]
    fn session_selection_clears_when_the_filter_changes() {
        let mut app = session_click_app();
        let _ = app.update(Message::SelectSession(0));
        assert_eq!(app.selected_profile, Some(0));

        let _ = app.update(Message::Search("beta".into()));
        assert_eq!(app.selected_profile, None);
        assert!(app.selected_profile_key.is_none());
        assert!(app.session_click.is_none());
        assert_eq!(app.profile_matches, vec![1]);

        // A rebuilt profile list must not leave the old index selected.
        let _ = app.update(Message::Search(String::new()));
        let _ = app.update(Message::SelectSession(1));
        app.profiles.insert(
            0,
            Session {
                name: "ahead".into(),
                host: "ahead.example.invalid".into(),
                ..Session::default()
            },
        );
        app.refresh_profile_matches();
        assert_eq!(app.selected_profile, None);
        assert!(app.selected_profile_key.is_none());
    }

    #[test]
    fn collapsing_a_session_group_hides_its_rows_until_a_filter_matches() {
        assert!(session_group_hides_rows(true, ""));
        assert!(session_group_hides_rows(true, "   "));
        assert!(!session_group_hides_rows(false, ""));
        assert!(!session_group_hides_rows(true, "beta"));

        let mut app = session_click_app();
        app.profiles[0].folder = "Work".into();
        app.refresh_profile_matches();
        let _ = app.update(Message::ToggleSessionGroup("Work".into()));
        assert!(app.collapsed_groups.contains("Work"));
        assert!(session_group_hides_rows(
            app.collapsed_groups.contains("Work"),
            &app.query,
        ));

        // The collapse flag stays, but a matching filter shows the rows.
        let _ = app.update(Message::Search("alpha".into()));
        assert!(app.collapsed_groups.contains("Work"));
        assert!(!session_group_hides_rows(
            app.collapsed_groups.contains("Work"),
            &app.query,
        ));

        let _ = app.update(Message::ToggleSessionGroup("Work".into()));
        assert!(!app.collapsed_groups.contains("Work"));
        assert!(!session_group_hides_rows(false, ""));
    }

    #[test]
    fn remote_context_menu_delete_opens_confirmation_without_deleting() {
        let mut app = app_with_one_pane_and_auxiliary_shell("filer-context-delete");
        app.files.remote_dir = "/srv".into();
        let entry = sftp::RemoteEntry::new("notes.txt", false, Some(4));
        app.files.remote_entries = vec![entry.clone()];
        let _ = app.update(Message::OpenRemoteFileContext {
            entry: entry.clone(),
            open_path: "/srv/notes.txt".into(),
        });
        let _ = app.update(Message::FileContextChoose(FileContextAction::Delete));
        assert!(
            app.context_menu.is_none(),
            "choosing an item closes the menu"
        );
        assert!(
            matches!(
                &app.dialog,
                Some(Dialog::FileDeleteRemote {
                    path,
                    directory,
                    ..
                }) if path == "/srv/notes.txt" && !*directory
            ),
            "Delete must open the existing confirmation, dialog={:?}",
            app.dialog.as_ref().map(|dialog| match dialog {
                Dialog::FileDeleteRemote {
                    path, directory, ..
                } => {
                    format!("delete {path} dir={directory}")
                }
                _ => "other".into(),
            })
        );
        assert_eq!(app.files.remote_entries, vec![entry]);
    }

    #[test]
    fn file_context_anchor_stays_inside_the_window() {
        let window = iced::Size::new(1280.0, 800.0);
        let menu = context_menu_popup_size(6);
        let bottom_limit = window.height - theme::metrics::STATUS_BAR_HEIGHT - 2.0;

        // A click on the bottom dock must sit above the status bar.
        let low = clamp_context_menu_anchor(iced::Point::new(135.0, 655.0), window, menu);
        assert!(
            low.y + menu.height <= bottom_limit,
            "menu bottom {} exceeds {}",
            low.y + menu.height,
            bottom_limit
        );
        assert!(low.y >= theme::metrics::MENU_BAR_HEIGHT);
        assert!(low.x >= 2.0);
        assert_eq!(low.x, 135.0);

        // Near the right edge the whole menu, border included, stays inside.
        let right = clamp_context_menu_anchor(iced::Point::new(1270.0, 400.0), window, menu);
        assert!(right.x >= 2.0);
        assert!(
            right.x + menu.width <= window.width - 2.0,
            "menu right {} exceeds {}",
            right.x + menu.width,
            window.width - 2.0
        );
        assert_eq!(right.y, 400.0);

        let inside = iced::Point::new(135.0, 400.0);
        assert_eq!(
            clamp_context_menu_anchor(inside, window, menu),
            inside,
            "an anchor already inside the free region is unchanged"
        );
    }

    fn local_context_labels(is_dir: bool) -> Vec<&'static str> {
        file_context_entries(&FileContextMenu {
            side: FileContextSide::Local,
            is_dir,
            open_path: "/tmp/item".into(),
            local_path: None,
            anchor: iced::Point::ORIGIN,
        })
        .into_iter()
        .map(|(label, _)| label)
        .collect()
    }

    #[test]
    fn local_context_menu_selects_the_row_and_escape_closes_it() {
        let mut app = app_with_one_pane_and_auxiliary_shell("explorer-context-select");
        let local_dir = app.files.local_dir.clone();
        let path = std::path::PathBuf::from("/tmp/notes.txt");
        let _ = app.update(Message::PointerMoved(iced::Point::new(30.0, 40.0)));
        let _ = app.update(Message::OpenLocalFileContext {
            path: path.clone(),
            is_dir: false,
        });
        assert!(app.context_menu.is_some(), "right-click opens the menu");
        assert_eq!(
            app.context_menu
                .as_ref()
                .and_then(ContextMenu::file)
                .map(|menu| (menu.side, menu.anchor.x, menu.anchor.y)),
            Some((FileContextSide::Local, 30.0, 40.0))
        );
        assert_eq!(app.files.selected_local.as_deref(), Some(path.as_path()));
        assert_eq!(
            local_context_labels(false),
            vec!["Upload", "New Folder", "Rename", "Delete", "Refresh"]
        );
        let _ = app.update(pressed_key(
            keyboard::Key::Named(keyboard::key::Named::Escape),
            keyboard::Modifiers::default(),
        ));
        assert!(app.context_menu.is_none(), "Escape closes the context menu");
        assert!(
            app.pty_write_log.is_empty(),
            "Escape must not reach the terminal"
        );

        let dir = std::path::PathBuf::from("/tmp/src");
        let _ = app.update(Message::OpenLocalFileContext {
            path: dir.clone(),
            is_dir: true,
        });
        assert_eq!(
            app.files.local_dir, local_dir,
            "right-click must not navigate"
        );
        assert_eq!(app.files.selected_local.as_deref(), Some(dir.as_path()));
        assert_eq!(
            local_context_labels(true),
            vec!["Open", "New Folder", "Rename", "Delete", "Refresh"]
        );
        let open = file_context_action_message(
            app.context_menu
                .as_ref()
                .and_then(ContextMenu::file)
                .expect("menu"),
            FileContextAction::Open,
        );
        assert!(matches!(
            open,
            Some(Message::FilesOpenLocal(path)) if path == dir
        ));
    }

    #[test]
    fn directory_context_menu_omits_transfer_actions() {
        let remote_file = remote_context_labels(false);
        assert_eq!(
            remote_file,
            vec![
                "Download",
                "Edit",
                "New Folder",
                "Rename",
                "Delete",
                "Refresh",
            ]
        );
        let remote_dir = remote_context_labels(true);
        assert_eq!(
            remote_dir,
            vec!["Open", "New Folder", "Rename", "Delete", "Refresh"]
        );
        assert!(!remote_dir.contains(&"Download"));
        assert_eq!(
            file_context_menu_height(remote_dir.len()),
            remote_dir.len() as f32 * FILE_CONTEXT_ITEM_HEIGHT + 2.0
        );
        assert!(
            file_context_menu_height(remote_dir.len())
                < file_context_menu_height(remote_file.len())
        );

        let local_file = local_context_labels(false);
        assert_eq!(
            local_file,
            vec!["Upload", "New Folder", "Rename", "Delete", "Refresh"]
        );
        let local_dir = local_context_labels(true);
        assert_eq!(
            local_dir,
            vec!["Open", "New Folder", "Rename", "Delete", "Refresh"]
        );
        assert!(!local_dir.contains(&"Upload"));
        assert_eq!(
            file_context_menu_height(local_dir.len()),
            local_dir.len() as f32 * FILE_CONTEXT_ITEM_HEIGHT + 2.0
        );
        // Open replaces Upload, so the local popup stays the same height.
        // Remote drops Download and Edit and adds only Open, so it shrinks.
        assert_eq!(
            file_context_menu_height(local_dir.len()),
            file_context_menu_height(local_file.len())
        );
    }

    #[test]
    fn local_context_menu_blocks_terminal_keys_and_chords() {
        let (mut app, first_id, _second_id) = arm_synced_pair("explorer-context-keys");
        let focus_mode = app.focus_mode;
        let _ = app.update(Message::OpenLocalFileContext {
            path: std::path::PathBuf::from("/tmp/notes.txt"),
            is_dir: false,
        });
        assert!(app.context_menu.is_some());
        app.pty_write_log.clear();
        let status = app.status.clone();
        let _ = app.update(pressed_char("x"));
        let _ = app.update(Message::KeyChord(KeyChord::FocusMode));
        let _ = app.update(Message::TerminalImeCommit(first_id, "x".into()));
        let _ = app.update(Message::RequestPaste);
        assert!(
            app.pty_write_log.is_empty(),
            "open context menu must not write to a pane, log={:?}",
            app.pty_write_log
        );
        assert_eq!(app.focus_mode, focus_mode, "chords must not run");
        assert_eq!(app.status, status, "paste must not start");
        assert!(
            app.context_menu.is_some(),
            "non-Escape keys leave the menu open"
        );
    }

    #[test]
    fn local_context_menu_delete_opens_confirmation_without_deleting() {
        let mut app = app_with_one_pane_and_auxiliary_shell("explorer-context-delete");
        let path = std::path::PathBuf::from("/tmp/notes.txt");
        app.files.local_entries = vec![listing_entry("notes.txt", false, Some(4), None)];
        app.files.local_entries[0].path = path.clone();
        let _ = app.update(Message::OpenLocalFileContext {
            path: path.clone(),
            is_dir: false,
        });
        let _ = app.update(Message::FileContextChoose(FileContextAction::Delete));
        assert!(
            app.context_menu.is_none(),
            "choosing an item closes the menu"
        );
        assert!(
            matches!(&app.dialog, Some(Dialog::FileDeleteLocal(pending)) if pending == &path),
            "Delete must open the existing confirmation"
        );
        assert_eq!(app.files.local_entries.len(), 1);
        assert_eq!(app.files.local_entries[0].path, path);
    }

    #[test]
    fn session_right_click_selects_opens_menu_and_opens_no_tab() {
        let mut app = session_click_app();
        let _ = app.update(Message::PointerMoved(iced::Point::new(20.0, 36.0)));
        let _ = app.update(Message::OpenSessionContext { index: 1 });
        let key = session_profile_key(&app.profiles[1]);
        assert_eq!(app.selected_profile, Some(1));
        assert_eq!(app.selected_profile_key.as_deref(), Some(key.as_str()));
        assert!(
            matches!(
                &app.context_menu,
                Some(ContextMenu::Session {
                    index: 1,
                    key: stored,
                    anchor,
                }) if stored == &key && anchor.x == 20.0 && anchor.y == 36.0
            ),
            "right-click opens the session menu at the pointer"
        );
        assert!(app.tabs.is_empty(), "right-click must not open a tab");
        assert!(
            app.session_click.is_none(),
            "right-click must not arm the double-click timer"
        );
        let labels: Vec<_> = session_context_entries()
            .into_iter()
            .map(|(label, _)| label)
            .collect();
        assert_eq!(labels, vec!["Open", "Edit", "New Session", "Import"]);

        // A second right-click, immediately, still opens nothing.
        let _ = app.update(Message::OpenSessionContext { index: 1 });
        assert!(app.tabs.is_empty());
        assert!(app.context_menu.is_some());
        assert!(app.session_click.is_none());

        let _ = app.update(pressed_key(
            keyboard::Key::Named(keyboard::key::Named::Escape),
            keyboard::Modifiers::default(),
        ));
        assert!(app.context_menu.is_none(), "Escape closes the context menu");
        assert!(app.tabs.is_empty());
    }

    #[test]
    fn session_right_click_clears_a_pending_double_click() {
        let mut app = session_click_app();
        let _ = app.update(Message::SelectSession(1));
        assert!(app.session_click.is_some());
        assert!(app.tabs.is_empty());
        let _ = app.update(Message::OpenSessionContext { index: 1 });
        assert!(
            app.tabs.is_empty(),
            "right-click must not open even inside the double-click window"
        );
        assert!(
            app.session_click.is_none(),
            "right-click must clear the pending double-click"
        );
        assert!(matches!(
            app.context_menu,
            Some(ContextMenu::Session { index: 1, .. })
        ));
    }

    #[test]
    fn session_left_a_right_b_left_b_does_not_open() {
        let mut app = session_click_app();
        let _ = app.update(Message::SelectSession(0));
        let _ = app.update(Message::OpenSessionContext { index: 1 });
        let _ = app.update(Message::FileContextDismiss);
        assert!(app.context_menu.is_none());
        let _ = app.update(Message::SelectSession(1));
        assert!(app.tabs.is_empty(), "B's only left click must not open");
        let key_b = session_profile_key(&app.profiles[1]);
        assert!(
            app.session_click
                .as_ref()
                .is_some_and(|(_, key)| key == &key_b),
            "the pending click must be armed for B"
        );
        let _ = app.update(Message::SelectSession(1));
        assert_eq!(app.tabs.len(), 1);
    }

    #[test]
    fn session_left_a_left_b_does_not_open() {
        let mut app = session_click_app();
        let _ = app.update(Message::SelectSession(0));
        let _ = app.update(Message::SelectSession(1));
        assert!(app.tabs.is_empty());
        let key_b = session_profile_key(&app.profiles[1]);
        assert!(
            app.session_click
                .as_ref()
                .is_some_and(|(_, key)| key == &key_b)
        );
    }

    #[test]
    fn context_menu_anchor_uses_pointer_message_delivered_before_press() {
        let mut app = session_click_app();
        let _ = app.update(Message::PointerMoved(iced::Point::new(400.0, 300.0)));
        let _ = app.update(Message::PointerMoved(iced::Point::new(12.0, 18.0)));
        let _ = app.update(Message::OpenSessionContext { index: 0 });
        assert_eq!(
            app.context_menu.as_ref().map(ContextMenu::anchor),
            Some(iced::Point::new(12.0, 18.0))
        );

        let mut app = app_with_one_pane_and_auxiliary_shell("filer-context-anchor-order");
        let file = sftp::RemoteEntry::new("notes.txt", false, Some(12));
        app.files.remote_entries = vec![file.clone()];
        let _ = app.update(Message::PointerMoved(iced::Point::new(400.0, 300.0)));
        let _ = app.update(Message::PointerMoved(iced::Point::new(22.0, 28.0)));
        let _ = app.update(Message::OpenRemoteFileContext {
            entry: file,
            open_path: "/srv/notes.txt".into(),
        });
        assert_eq!(
            app.context_menu.as_ref().map(ContextMenu::anchor),
            Some(iced::Point::new(22.0, 28.0))
        );
    }

    #[test]
    fn session_context_open_opens_exactly_one_tab() {
        let mut app = session_click_app();
        let _ = app.update(Message::OpenSessionContext { index: 0 });
        assert!(app.tabs.is_empty());
        let _ = app.update(Message::SessionContextChoose(SessionContextAction::Open));
        assert_eq!(app.tabs.len(), 1);
        assert!(app.context_menu.is_none());
        let _ = app.update(Message::SessionContextChoose(SessionContextAction::Open));
        assert_eq!(
            app.tabs.len(),
            1,
            "a second choose with no menu opens nothing"
        );
    }

    #[test]
    fn session_context_edit_opens_connection_dialog_for_that_profile() {
        let mut app = session_click_app();
        let key = session_profile_key(&app.profiles[1]);
        let _ = app.update(Message::OpenSessionContext { index: 1 });
        let _ = app.update(Message::SessionContextChoose(SessionContextAction::Edit));
        assert!(matches!(app.dialog, Some(Dialog::Connection)));
        assert_eq!(app.editing_profile.as_deref(), Some(key.as_str()));
        assert!(app.tabs.is_empty());
        assert!(app.context_menu.is_none());
    }

    #[test]
    fn session_context_new_and_import_dispatch_the_session_menu_commands() {
        let mut app = session_click_app();
        let _ = app.update(Message::OpenSessionContext { index: 0 });
        let _ = app.update(Message::SessionContextChoose(
            SessionContextAction::NewSession,
        ));
        assert!(matches!(app.dialog, Some(Dialog::Connection)));
        assert!(
            app.editing_profile.is_none(),
            "New Session is a new connection, not an edit"
        );
        assert!(app.tabs.is_empty());
        assert!(app.context_menu.is_none());

        app.dialog = None;
        let _ = app.update(Message::OpenSessionContext { index: 0 });
        let _ = app.update(Message::SessionContextChoose(SessionContextAction::Import));
        assert!(matches!(app.dialog, Some(Dialog::ImportProfiles)));
        assert!(app.context_menu.is_none());
    }

    #[test]
    fn session_context_stale_key_does_nothing() {
        let mut app = session_click_app();
        let _ = app.update(Message::OpenSessionContext { index: 0 });
        app.profiles[0].name = "renamed".into();
        let _ = app.update(Message::SessionContextChoose(SessionContextAction::Open));
        assert!(
            app.context_menu.is_none(),
            "a stale choice still closes the menu"
        );
        assert!(app.tabs.is_empty(), "a stale key must not open a tab");
        assert!(app.dialog.is_none());

        let _ = app.update(Message::OpenSessionContext { index: 1 });
        app.profiles.remove(1);
        let _ = app.update(Message::SessionContextChoose(SessionContextAction::Edit));
        assert!(app.dialog.is_none());
        assert!(app.editing_profile.is_none());
        assert!(app.context_menu.is_none());

        let _ = app.update(Message::OpenSessionContext { index: 0 });
        app.profiles[0].name = "moved".into();
        let _ = app.update(Message::SessionContextChoose(
            SessionContextAction::NewSession,
        ));
        assert!(
            app.dialog.is_none(),
            "a stale menu must not dispatch New Session"
        );
        assert!(app.tabs.is_empty());
    }

    #[test]
    fn session_context_menu_blocks_terminal_keys_and_chords() {
        let (mut app, first_id, _second_id) = arm_synced_pair("session-context-keys");
        app.profiles = vec![Session {
            name: "alpha".into(),
            host: "alpha.example.invalid".into(),
            ..Session::default()
        }];
        app.refresh_profile_matches();
        let focus_mode = app.focus_mode;
        let _ = app.update(Message::OpenSessionContext { index: 0 });
        assert!(app.context_menu.is_some());
        app.pty_write_log.clear();
        let status = app.status.clone();
        let _ = app.update(pressed_char("x"));
        let _ = app.update(pressed_key(
            keyboard::Key::Character("x".into()),
            keyboard::Modifiers::default(),
        ));
        let _ = app.update(Message::KeyChord(KeyChord::FocusMode));
        let _ = app.update(Message::TerminalImeCursor(
            first_id,
            iced::Rectangle::default(),
        ));
        let _ = app.update(Message::TerminalImeCommit(first_id, "x".into()));
        let _ = app.update(Message::RequestPaste);
        assert!(
            app.pty_write_log.is_empty(),
            "open context menu must not write to a pane, log={:?}",
            app.pty_write_log
        );
        assert_eq!(app.focus_mode, focus_mode, "chords must not run");
        assert_eq!(app.status, status, "paste must not start");
        assert!(
            app.context_menu.is_some(),
            "non-Escape keys leave the menu open"
        );
        assert!(app.ime_cursor.is_none(), "IME target must stay disabled");

        app.context_menu = None;
        app.pty_write_log.clear();
        let _ = app.update(pressed_key(
            keyboard::Key::Named(keyboard::key::Named::Enter),
            keyboard::Modifiers::default(),
        ));
        assert!(
            app.pty_write_log.iter().any(|(id, _)| *id == first_id),
            "recorder must see Enter once the menu is gone, log={:?}",
            app.pty_write_log
        );
    }

    fn app_with_focused_pane(label: &str) -> (App, u64) {
        let path = std::env::temp_dir().join(format!(
            "inspirum-find-{label}-{}-nonexistent.json",
            std::process::id()
        ));
        let mut app = App::boot(path, None);
        let profile = Session {
            name: "find fixture".into(),
            host: "example.invalid".into(),
            ..Session::default()
        };
        let pane = app.new_terminal_pane(profile.clone());
        let id = pane.id;
        app.tabs.push(Workspace::new(profile, pane));
        (app, id)
    }

    #[test]
    fn find_chord_opens_strip_for_the_focused_pane() {
        let (mut app, id) = app_with_focused_pane("chord-open");
        let focus = app.tabs[0].focus;
        let _ = app.update(Message::KeyChord(KeyChord::Find));
        assert!(app.find_open, "chord must open the strip");
        assert!(app.find_focused, "opening the strip focuses its input");
        assert_eq!(
            app.tools.history_owner,
            Some(id),
            "owner must be the focused pane"
        );
        assert_eq!(app.tabs[0].focus, focus, "chord must not move pane focus");
    }

    #[test]
    fn find_chord_does_nothing_while_a_menu_is_open() {
        let (mut app, _id) = app_with_focused_pane("chord-menu");
        for (open_menu, dock_menu) in [
            (Some(menu::Menu::Edit), None),
            (None, Some(dock::Area::Bottom)),
        ] {
            app.open_menu = open_menu;
            app.dock_menu = dock_menu;
            app.find_open = false;
            app.find_focused = false;
            app.tools.history_owner = None;
            let _ = app.update(Message::KeyChord(KeyChord::Find));
            assert!(!app.find_open, "menu must drop the Find chord");
            assert!(
                app.tools.history_owner.is_none(),
                "a dropped chord must not retarget history"
            );
            assert!(
                app.open_menu.is_some() || app.dock_menu.is_some(),
                "the chord must not close the menu"
            );
        }
        app.open_menu = None;
        app.dock_menu = None;
        app.privacy_locked = true;
        let _ = app.update(Message::KeyChord(KeyChord::Find));
        assert!(!app.find_open, "privacy lock must drop the Find chord");
    }

    #[test]
    fn find_focus_blocks_key_and_paste_until_escape_releases_the_terminal() {
        let (mut app, id) = app_with_focused_pane("chord-keys");
        app.find_open = true;
        app.find_focused = true;
        app.pty_write_log.clear();
        let _ = app.update(pressed_char("x"));
        let _ = app.update(pressed_key(
            keyboard::Key::Character("x".into()),
            keyboard::Modifiers::default(),
        ));
        let _ = app.update(Message::ClipboardRead(id, vec![id], Some("pasted".into())));
        let _ = app.update(Message::RequestPaste);
        assert!(
            app.pty_write_log.is_empty(),
            "Find focus must drop keys and paste, log={:?}",
            app.pty_write_log
        );
        assert_eq!(app.status, "Ready", "paste must not start a clipboard read");
        let _ = app.update(pressed_key(
            keyboard::Key::Named(keyboard::key::Named::Escape),
            keyboard::Modifiers::default(),
        ));
        assert!(!app.find_open, "Escape must close the strip");
        assert!(!app.find_focused, "Escape must clear Find focus");
        assert!(
            app.pty_write_log.is_empty(),
            "Find Escape must not be sent to the PTY, log={:?}",
            app.pty_write_log
        );
        app.pty_write_log.clear();
        let _ = app.update(pressed_char("z"));
        assert!(
            app.pty_write_log
                .iter()
                .any(|(pane, bytes)| *pane == id && bytes.as_slice() == b"z"),
            "a key after close must reach the terminal, log={:?}",
            app.pty_write_log
        );
    }

    #[test]
    fn find_query_and_next_advance_the_shared_history_hit() {
        let (mut app, id) = app_with_focused_pane("chord-hits");
        app.tools.history_owner = Some(id);
        app.tools
            .histories
            .entry(id)
            .or_default()
            .update_snapshot(vec![
                "alpha".into(),
                "needle one".into(),
                "beta".into(),
                "needle two".into(),
            ]);
        let _ = app.update(Message::FindQuery("needle".into()));
        let state = app.tools.histories.get(&id).expect("seeded history");
        assert_eq!(state.query(), "needle");
        assert_eq!(state.hits().len(), 2);
        assert_eq!(state.selected_hit_index(), 0);
        let _ = app.update(Message::FindNext);
        assert_eq!(
            app.tools
                .histories
                .get(&id)
                .expect("history")
                .selected_hit_index(),
            1
        );
        assert!(app.find_focused, "typing and Next keep Find focus");
    }

    #[test]
    fn focusing_a_pane_clears_find_focus_without_closing_the_strip() {
        let (mut app, _id) = app_with_focused_pane("chord-pane-focus");
        app.find_open = true;
        app.find_focused = true;
        let pane = app.tabs[0].focus;
        let _ = app.update(Message::Focus(pane));
        assert!(app.find_open, "pane focus must not close the strip");
        assert!(!app.find_focused, "pane focus must release the Find input");
    }

    #[test]
    fn find_click_back_then_captured_escape_closes_strip() {
        let (mut app, _id) = app_with_focused_pane("click-back-escape");
        let pane = app.tabs[0].focus;
        let _ = app.update(Message::FindOpen);
        let _ = app.update(Message::Focus(pane));
        assert!(!app.find_focused);
        let _ = app.update(Message::FindFocusSynced(true));
        assert!(app.find_focused);
        app.pty_write_log.clear();
        let _ = app.update(Message::DismissModalKey);
        assert!(!app.find_open);
        assert!(app.pty_write_log.is_empty());
    }

    #[test]
    fn find_focus_sync_false_releases_keys_to_terminal() {
        let (mut app, id) = app_with_focused_pane("sync-release");
        let _ = app.update(Message::FindOpen);
        let _ = app.update(Message::FindFocusSynced(false));
        let _ = app.update(pressed_char("x"));
        assert!(
            app.pty_write_log
                .iter()
                .any(|(pane, bytes)| *pane == id && bytes.as_slice() == b"x")
        );
    }

    #[test]
    fn find_nav_after_pane_click_reclaims_keyboard() {
        let (mut app, _id) = app_with_focused_pane("nav-reclaim");
        let pane = app.tabs[0].focus;
        let _ = app.update(Message::FindOpen);
        let _ = app.update(Message::Focus(pane));
        let _ = app.update(Message::FindNext);
        assert!(app.find_focused);
        app.pty_write_log.clear();
        let _ = app.update(pressed_char("x"));
        assert!(app.pty_write_log.is_empty());
    }

    #[test]
    fn find_dialog_dismiss_keeps_prior_owner() {
        for pane_owned in [false, true] {
            let (mut app, id) = app_with_focused_pane("dialog-owner");
            let _ = app.update(Message::FindOpen);
            if pane_owned {
                let pane = app.tabs[0].focus;
                let _ = app.update(Message::Focus(pane));
            }
            let _ = app.update(Message::Commands);
            let _ = app.update(Message::CloseDialog);
            assert_eq!(app.find_focused, !pane_owned);
            app.pty_write_log.clear();
            let _ = app.update(pressed_char("x"));
            assert_eq!(
                app.pty_write_log
                    .iter()
                    .any(|(pane, bytes)| *pane == id && bytes.as_slice() == b"x"),
                pane_owned
            );
        }
    }

    #[test]
    fn find_tab_switch_and_lock_close_strip() {
        let (mut app, _id) = app_with_focused_pane("tab-lock");
        let profile = Session {
            name: "second".into(),
            host: "other.invalid".into(),
            ..Session::default()
        };
        let pane = app.new_terminal_pane(profile.clone());
        app.tabs.push(Workspace::new(profile, pane));
        let _ = app.update(Message::FindOpen);
        let _ = app.update(Message::SelectTab(1));
        assert!(!app.find_open);
        assert!(!app.find_focused);
        let _ = app.update(Message::FindOpen);
        let _ = app.update(Message::TogglePrivacyLock);
        assert!(!app.find_open);
        assert!(!app.find_focused);
    }

    #[test]
    fn find_captured_escape_ignores_draft_in_another_tab() {
        let (mut app, first_id) = app_with_focused_pane("remote-draft-escape");
        let _ = app.update(Message::ToggleFreeType(first_id));
        app.free_type.insert(
            first_id,
            text_editor::Content::with_text("unsent draft in tab A"),
        );
        let profile = Session {
            name: "second".into(),
            host: "other.invalid".into(),
            ..Session::default()
        };
        let pane = app.new_terminal_pane(profile.clone());
        let second_id = pane.id;
        app.tabs.push(Workspace::new(profile, pane));
        let _ = app.update(Message::SelectTab(1));
        let _ = app.update(Message::FindOpen);
        assert_eq!(app.tools.history_owner, Some(second_id));
        assert!(app.find_open && app.find_focused);

        app.pty_write_log.clear();
        let _ = app.update(Message::DismissModalKey);
        assert!(!app.find_open, "a remote draft must not veto Find Escape");
        assert!(!app.find_focused);
        assert_eq!(
            app.free_type.get(&first_id).map(|draft| draft.text()),
            Some("unsent draft in tab A".into())
        );
        assert!(app.pty_write_log.is_empty());

        // A delayed focus probe may have released the flag; the tab-A draft
        // still cannot claim a captured Escape from tab B.
        let _ = app.update(Message::FindOpen);
        let _ = app.update(Message::FindFocusSynced(false));
        let _ = app.update(Message::DismissModalKey);
        assert!(!app.find_open);
        assert_eq!(
            app.free_type.get(&first_id).map(|draft| draft.text()),
            Some("unsent draft in tab A".into())
        );
        assert!(app.pty_write_log.is_empty());
    }

    #[test]
    fn find_stale_flag_escape_respects_sender_and_focused_free_type() {
        let (mut app, id) = app_with_focused_pane("stale-escape");
        let _ = app.update(Message::FindOpen);
        let _ = app.update(Message::FindFocusSynced(false));
        app.sender_focused = true;
        let _ = app.update(Message::DismissModalKey);
        assert!(app.find_open);
        assert!(!app.sender_focused);
        let _ = app.update(Message::ToggleFreeType(id));
        assert!(app.free_type.contains_key(&id));
        let _ = app.update(Message::DismissModalKey);
        assert!(app.find_open, "focused free-type editor owns Escape");
        let _ = app.update(Message::FindFocusSynced(true));
        let _ = app.update(Message::DismissModalKey);
        assert!(!app.find_open, "Find focus owns Escape despite a draft");
        assert!(app.free_type.contains_key(&id));
        assert!(app.pty_write_log.is_empty());
    }

    #[test]
    fn find_focus_mode_and_tab_close_drop_strip() {
        let (mut app, _id) = app_with_focused_pane("mode-close");
        let _ = app.update(Message::FindOpen);
        let _ = app.update(Message::ToggleFocusMode);
        assert!(!app.find_open);
        assert!(!app.find_focused);
        let _ = app.update(Message::ToggleFocusMode);
        let _ = app.update(Message::FindOpen);
        let _ = app.update(Message::ConfirmClose(0));
        assert!(!app.find_open);
        assert!(!app.find_focused);
    }

    #[test]
    fn find_counter_is_danger_only_when_a_query_misses() {
        let (label, danger) = super::find_counter("", 0, 0);
        assert_eq!(label, "0/0");
        assert!(!danger, "an empty query with no hits stays normal");
        let (label, danger) = super::find_counter("needle", 0, 0);
        assert_eq!(label, "0/0");
        assert!(danger, "a miss must use the danger colour");
        let (label, danger) = super::find_counter("needle", 1, 2);
        assert_eq!(label, "2/2");
        assert!(!danger, "a hit must stay normal");
        let (label, danger) = super::find_counter("", 0, 3);
        assert_eq!(label, "1/3");
        assert!(!danger);
        assert_eq!(super::FIND_COUNTER_MIN.chars().count(), 7);
    }
}
