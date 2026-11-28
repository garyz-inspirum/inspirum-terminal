pub mod settings;

use crate::theme::TerminalTheme;
use crate::types::Size;
use alacritty_terminal::event::{Event, EventListener, Notify, OnResize, WindowSize};
use alacritty_terminal::event_loop::{EventLoop, Msg, Notifier};
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Column, Direction, Line, Point, Side};
use alacritty_terminal::selection::{
    Selection, SelectionRange, SelectionType as AlacrittySelectionType,
};
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::search::{Match, RegexIter, RegexSearch};
use alacritty_terminal::term::{
    self, cell::Cell, test::TermSize, viewport_to_point, Term, TermMode,
};
use alacritty_terminal::{tty, Grid};
use settings::BackendSettings;
use std::borrow::Cow;
use std::cmp::min;
use std::io::Result;
use std::ops::{Index, RangeInclusive};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{mpsc, Arc};
use std::thread::{self, JoinHandle};
use std::time::Duration;

pub type TerminalMode = TermMode;
pub type PtyEvent = Event;
pub type SelectionType = AlacrittySelectionType;

/// Serializes the executable token for Alacritty's Windows command line.
///
/// Alacritty 0.25 appends `Shell::program` verbatim while creating ConPTY with
/// a null application name, so the first command-line token must be quoted at
/// this boundary. Quotes and NUL cannot be represented safely as part of a
/// Windows executable path in that command line.
#[doc(hidden)]
pub fn serialize_windows_program(program: &str) -> Result<String> {
    if program.contains('\0') || program.contains('"') {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "Windows PTY executable path contains a NUL or quote",
        ));
    }

    Ok(format!("\"{program}\""))
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MouseModifiers {
    pub shift: bool,
    pub alt: bool,
    pub command: bool,
}

#[derive(Debug, Clone)]
pub enum BackendCommand {
    Write(Vec<u8>),
    Scroll(i32),
    Resize(Size, Size),
    SelectStart(SelectionType, f32, f32),
    SelectUpdate(f32, f32),
    ProcessLink(LinkAction, Point),
    MouseReportAt(MouseButton, MouseModifiers, f32, f32, bool),
}

#[derive(Debug, Clone)]
pub enum MouseMode {
    Sgr,
    Normal(bool),
}

impl From<TermMode> for MouseMode {
    fn from(term_mode: TermMode) -> Self {
        if term_mode.contains(TermMode::SGR_MOUSE) {
            MouseMode::Sgr
        } else if term_mode.contains(TermMode::UTF8_MOUSE) {
            MouseMode::Normal(true)
        } else {
            MouseMode::Normal(false)
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseButton {
    LeftButton = 0,
    MiddleButton = 1,
    RightButton = 2,
    LeftMove = 32,
    MiddleMove = 33,
    RightMove = 34,
    NoneMove = 35,
    ScrollUp = 64,
    ScrollDown = 65,
    Other = 99,
}

#[derive(Debug, Clone)]
pub enum LinkAction {
    Clear,
    Hover,
    Open,
}

#[derive(Clone, Copy, Debug)]
pub struct TerminalSize {
    pub cell_width: u16,
    pub cell_height: u16,
    num_cols: u16,
    num_lines: u16,
    layout_size: Size,
    /// Exact cell size in pixels. `cell_width`/`cell_height` are the
    /// truncated values the PTY window-size report needs; grid counts and
    /// pointer mapping must use these, or a fractional cell (8.4 px) is
    /// treated as 8 px and the grid ends up ~5% wider than the canvas.
    cell_size: Size,
}

impl Default for TerminalSize {
    fn default() -> Self {
        Self {
            cell_width: 1,
            cell_height: 1,
            num_cols: 80,
            num_lines: 50,
            layout_size: Size::default(),
            cell_size: Size::new(1.0, 1.0),
        }
    }
}

impl TerminalSize {
    /// Grid for a layout of `layout_size` pixels split into cells of
    /// `font_size` pixels; `None` when not even one cell fits.
    pub fn for_layout(layout_size: Size, font_size: Size) -> Option<Self> {
        if !(font_size.width > 0.0 && font_size.height > 0.0) {
            return None;
        }
        // Callers send `count * cell`; the epsilon absorbs the float
        // round-trip (153 * 8.4 / 8.4 = 152.99998) so it cannot lose a cell.
        let fit = |extent: f32, cell: f32| {
            (extent / cell + 1e-3).floor().clamp(0.0, u16::MAX as f32) as u16
        };
        let lines = fit(layout_size.height, font_size.height);
        let cols = fit(layout_size.width, font_size.width);
        (lines > 0 && cols > 0).then_some(Self {
            layout_size,
            cell_size: font_size,
            cell_height: font_size.height as u16,
            cell_width: font_size.width as u16,
            num_lines: lines,
            num_cols: cols,
        })
    }

    /// Grid width in cells, as told to the PTY.
    pub fn num_cols(&self) -> u16 {
        self.num_cols
    }

    /// Grid height in cells, as told to the PTY.
    pub fn num_lines(&self) -> u16 {
        self.num_lines
    }
}

impl Dimensions for TerminalSize {
    fn total_lines(&self) -> usize {
        self.screen_lines()
    }

    fn screen_lines(&self) -> usize {
        self.num_lines as usize
    }

    fn columns(&self) -> usize {
        self.num_cols as usize
    }

    fn last_column(&self) -> Column {
        Column(self.num_cols as usize - 1)
    }

    fn bottommost_line(&self) -> Line {
        Line(self.num_lines as i32 - 1)
    }
}

impl From<TerminalSize> for WindowSize {
    fn from(size: TerminalSize) -> Self {
        Self {
            num_lines: size.num_lines,
            num_cols: size.num_cols,
            cell_width: size.cell_width,
            cell_height: size.cell_height,
        }
    }
}

struct PtyEventLoopRollback(Option<Notifier>);

impl PtyEventLoopRollback {
    fn new(notifier: Notifier) -> Self {
        Self(Some(notifier))
    }

    fn commit(mut self) -> Notifier {
        self.0.take().expect("rollback notifier")
    }
}

impl Drop for PtyEventLoopRollback {
    fn drop(&mut self) {
        if let Some(notifier) = self.0.take() {
            let _ = notifier.0.send(Msg::Shutdown);
        }
    }
}

pub struct TerminalBackend {
    pub id: u64,
    pub url_regex: RegexSearch,
    term: Arc<FairMutex<Term<EventProxy>>>,
    size: TerminalSize,
    notifier: Notifier,
    last_content: RenderableContent,
    /// Where a Simple drag selection was pressed (see
    /// `start_drag_selection`); `None` for other selection types.
    selection_anchor: Option<DragAnchor>,
    subscription_shutdown: Arc<AtomicBool>,
    subscription_thread: Option<JoinHandle<()>>,
}

impl TerminalBackend {
    pub fn new(
        id: u64,
        pty_event_proxy_sender: Sender<(u64, PtyEvent)>,
        settings: BackendSettings,
    ) -> Result<Self> {
        Self::new_with_waker(id, pty_event_proxy_sender, settings, Arc::new(|| {}))
    }

    /// Construct the PTY/parser backend without coupling it to a particular GUI toolkit.
    ///
    /// The callback only schedules a frontend refresh. Terminal bytes, parsing, resize,
    /// process shutdown, and PTY ownership remain in this backend.
    pub fn new_with_waker(
        id: u64,
        pty_event_proxy_sender: Sender<(u64, PtyEvent)>,
        settings: BackendSettings,
        wake: Arc<dyn Fn() + Send + Sync>,
    ) -> Result<Self> {
        let event_sink: Arc<dyn Fn(u64, PtyEvent) + Send + Sync> = Arc::new(move |id, event| {
            let _ = pty_event_proxy_sender.send((id, event));
        });
        Self::new_with_event_sink_and_subscription_spawner(
            id,
            settings,
            event_sink,
            wake,
            |builder, subscription| builder.spawn(subscription),
        )
    }

    /// Construct the PTY/parser backend with a toolkit-neutral event sink.
    ///
    /// This is used by the Iced frontend to receive PTY lifecycle/output events
    /// without adding a polling loop or changing terminal/process ownership.
    pub fn new_with_event_sink(
        id: u64,
        settings: BackendSettings,
        event_sink: Arc<dyn Fn(u64, PtyEvent) + Send + Sync>,
    ) -> Result<Self> {
        Self::new_with_event_sink_and_subscription_spawner(
            id,
            settings,
            event_sink,
            Arc::new(|| {}),
            |builder, subscription| builder.spawn(subscription),
        )
    }

    #[doc(hidden)]
    pub fn new_with_subscription_spawner<F>(
        id: u64,
        pty_event_proxy_sender: Sender<(u64, PtyEvent)>,
        settings: BackendSettings,
        spawn_subscription: F,
    ) -> Result<Self>
    where
        F: FnOnce(thread::Builder, Box<dyn FnOnce() + Send + 'static>) -> Result<JoinHandle<()>>,
    {
        let wake: Arc<dyn Fn() + Send + Sync> = Arc::new(|| {});
        let event_sink: Arc<dyn Fn(u64, PtyEvent) + Send + Sync> = Arc::new(move |id, event| {
            let _ = pty_event_proxy_sender.send((id, event));
        });
        Self::new_with_event_sink_and_subscription_spawner(
            id,
            settings,
            event_sink,
            wake,
            spawn_subscription,
        )
    }

    fn new_with_event_sink_and_subscription_spawner<F>(
        id: u64,
        settings: BackendSettings,
        event_sink: Arc<dyn Fn(u64, PtyEvent) + Send + Sync>,
        wake: Arc<dyn Fn() + Send + Sync>,
        spawn_subscription: F,
    ) -> Result<Self>
    where
        F: FnOnce(thread::Builder, Box<dyn FnOnce() + Send + 'static>) -> Result<JoinHandle<()>>,
    {
        #[cfg(target_os = "windows")]
        let program = serialize_windows_program(&settings.shell)?;
        #[cfg(not(target_os = "windows"))]
        let program = settings.shell;
        let pty_config = tty::Options {
            shell: Some(tty::Shell::new(program, settings.args)),
            working_directory: settings.working_directory,
            #[cfg(target_os = "windows")]
            escape_args: true,
            ..tty::Options::default()
        };
        let config = term::Config {
            scrolling_history: SCROLLBACK_LINES,
            ..term::Config::default()
        };
        let terminal_size = TerminalSize::default();
        let pty = tty::new(&pty_config, terminal_size.into(), id)?;
        let (event_sender, event_receiver) = mpsc::channel();
        let event_proxy = EventProxy(event_sender);
        let mut term = Term::new(config, &terminal_size, event_proxy.clone());
        let initial_content = RenderableContent {
            grid: term.grid().clone(),
            selectable_range: None,
            terminal_mode: *term.mode(),
            terminal_size,
            cursor: term.grid_mut().cursor_cell().clone(),
            hovered_hyperlink: None,
        };
        let term = Arc::new(FairMutex::new(term));
        // Preserve final diagnostics even when a short-lived child exits before the read event.
        let pty_event_loop = EventLoop::new(term.clone(), event_proxy, pty, true, false)?;
        let event_loop_rollback = PtyEventLoopRollback::new(Notifier(pty_event_loop.channel()));
        let url_regex = RegexSearch::new(r#"(ipfs:|ipns:|magnet:|mailto:|gemini://|gopher://|https://|http://|news:|file://|git://|ssh:|ftp://)[^\u{0000}-\u{001F}\u{007F}-\u{009F}<>"\s{-}\^⟨⟩`]+"#).unwrap();
        let _pty_event_loop_thread = pty_event_loop.spawn();
        let subscription_shutdown = Arc::new(AtomicBool::new(false));
        let thread_shutdown = subscription_shutdown.clone();
        let subscription = Box::new(move || {
            while !thread_shutdown.load(Ordering::Acquire) {
                match event_receiver.recv_timeout(Duration::from_millis(50)) {
                    Ok(event) => {
                        event_sink(id, event.clone());
                        wake();
                        if let Event::Exit = event {
                            break;
                        }
                    }
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
                }
            }
        });
        let subscription_thread = spawn_subscription(
            thread::Builder::new().name(format!("pty_event_subscription_{}", id)),
            subscription,
        )?;
        let notifier = event_loop_rollback.commit();

        Ok(Self {
            id,
            url_regex,
            term: term.clone(),
            size: terminal_size,
            notifier,
            last_content: initial_content,
            selection_anchor: None,
            subscription_shutdown,
            subscription_thread: Some(subscription_thread),
        })
    }

    pub fn process_command(&mut self, cmd: BackendCommand) {
        let term = self.term.clone();
        let mut term = term.lock();
        match cmd {
            BackendCommand::Write(input) => {
                self.write(input);
                term.scroll_display(Scroll::Bottom);
            }
            BackendCommand::Scroll(delta) => {
                self.scroll(&mut term, delta);
            }
            BackendCommand::Resize(layout_size, font_size) => {
                self.resize(&mut term, layout_size, font_size);
            }
            BackendCommand::SelectStart(selection_type, x, y) => {
                self.start_selection(&mut term, selection_type, x, y);
            }
            BackendCommand::SelectUpdate(x, y) => {
                self.update_selection(&mut term, x, y);
            }
            BackendCommand::ProcessLink(link_action, point) => {
                self.process_link_action(&term, link_action, point);
            }
            BackendCommand::MouseReportAt(button, modifiers, x, y, pressed) => {
                let point = Self::selection_point(x, y, &self.size, term.grid().display_offset());
                self.process_mouse_report_flags(
                    button,
                    modifiers.shift,
                    modifiers.alt,
                    modifiers.command,
                    point,
                    pressed,
                );
            }
        };
    }

    pub fn selection_point(
        x: f32,
        y: f32,
        terminal_size: &TerminalSize,
        display_offset: usize,
    ) -> Point {
        let col = (x.max(0.0) / terminal_size.cell_size.width) as usize;
        let col = min(Column(col), Column(terminal_size.num_cols as usize - 1));

        let line = (y.max(0.0) / terminal_size.cell_size.height) as usize;
        let line = min(line, terminal_size.num_lines as usize - 1);

        viewport_to_point(display_offset, Point::new(line, col))
    }

    pub fn selectable_content(&self) -> String {
        let term = self.term.clone();
        let terminal = term.lock();
        terminal.selection_to_string().unwrap_or_default()
    }

    pub fn sync(&mut self) -> &RenderableContent {
        let term = self.term.clone();
        let mut terminal = term.lock();
        let selectable_range = match &terminal.selection {
            Some(s) => s.to_range(&terminal),
            None => None,
        };

        let cursor = terminal.grid_mut().cursor_cell().clone();
        self.last_content.grid = terminal.grid().clone();
        self.last_content.selectable_range = selectable_range;
        self.last_content.cursor = cursor.clone();
        self.last_content.terminal_mode = *terminal.mode();
        self.last_content.terminal_size = self.size;
        self.last_content()
    }

    pub fn last_content(&self) -> &RenderableContent {
        &self.last_content
    }

    /// Return the currently visible terminal grid with resolved colors and text attributes.
    ///
    /// This keeps toolkit-specific rendering outside the PTY/parser backend while preserving the
    /// exact Alacritty cell model. The supplied theme controls ANSI/named-color resolution.
    pub fn display_snapshot(&mut self, theme: &TerminalTheme) -> DisplaySnapshot {
        // The Iced renderer needs only the viewport, not a clone of all
        // historical scrollback. sync() materializes the entire Alacritty
        // Grid (potentially hundreds of thousands of cells) on every frame.
        // Hold the parser lock only while extracting bounded visible cells.
        // Consumers needing the full retained grid must explicitly call sync().
        let term = self.term.clone();
        let mut terminal = term.lock();
        let selectable_range = terminal
            .selection
            .as_ref()
            .and_then(|selection| selection.to_range(&terminal));
        let mode = *terminal.mode();
        let cursor_cell = terminal.grid_mut().cursor_cell().clone();
        self.last_content.terminal_mode = mode;
        self.last_content.terminal_size = self.size;
        self.last_content.cursor = cursor_cell.clone();
        self.last_content.selectable_range = selectable_range;
        let grid = terminal.grid();
        let terminal_size = self.size;
        let global_bg = theme.get_color(alacritty_terminal::vte::ansi::Color::Named(
            alacritty_terminal::vte::ansi::NamedColor::Background,
        ));
        let [bg_r, bg_g, bg_b, _] = global_bg.to_array();
        let mut cells =
            Vec::with_capacity(terminal_size.num_cols as usize * terminal_size.num_lines as usize);

        for indexed in grid.display_iter() {
            let flags = indexed.cell.flags;
            if flags.contains(term::cell::Flags::WIDE_CHAR_SPACER) {
                continue;
            }

            let line = indexed.point.line.0 + grid.display_offset() as i32;
            if line < 0 || line >= terminal_size.num_lines as i32 {
                continue;
            }

            let selected = selectable_range.is_some_and(|range| range.contains(indexed.point));
            let inverse = flags.contains(term::cell::Flags::INVERSE);
            let dim = flags.intersects(term::cell::Flags::DIM | term::cell::Flags::DIM_BOLD);

            let mut foreground = theme.get_color(indexed.fg);
            let mut background = theme.get_color(indexed.bg);
            if dim {
                foreground = foreground.linear_multiply(0.7);
            }
            if inverse || selected {
                std::mem::swap(&mut foreground, &mut background);
            }

            // Full-screen applications use DECTCEM to hide the cursor. The
            // live input cursor also must not be painted over scrollback.
            let cursor = mode.contains(TermMode::SHOW_CURSOR)
                && grid.display_offset() == 0
                && grid.cursor.point == indexed.point;
            let cursor_color = theme.get_color(cursor_cell.fg);
            let [fg_r, fg_g, fg_b, _] = foreground.to_array();
            let [cell_bg_r, cell_bg_g, cell_bg_b, _] = background.to_array();
            let [cursor_r, cursor_g, cursor_b, _] = cursor_color.to_array();

            cells.push(DisplayCell {
                character: if flags.contains(term::cell::Flags::HIDDEN) {
                    ' '
                } else {
                    indexed.c
                },
                row: line as usize,
                column: indexed.point.column.0,
                foreground: [fg_r, fg_g, fg_b],
                background: [cell_bg_r, cell_bg_g, cell_bg_b],
                cursor_color: [cursor_r, cursor_g, cursor_b],
                bold: flags.intersects(term::cell::Flags::BOLD | term::cell::Flags::DIM_BOLD),
                italic: flags
                    .intersects(term::cell::Flags::ITALIC | term::cell::Flags::BOLD_ITALIC),
                underline: flags.intersects(term::cell::Flags::ALL_UNDERLINES),
                strikeout: flags.contains(term::cell::Flags::STRIKEOUT),
                wide: flags.contains(term::cell::Flags::WIDE_CHAR),
                cursor,
            });
        }

        let rows = terminal_size.num_lines as usize;
        let row_ranges = display_row_ranges(rows, &cells);

        DisplaySnapshot {
            rows,
            columns: terminal_size.num_cols as usize,
            background: [bg_r, bg_g, bg_b],
            row_ranges,
            cells,
        }
    }

    /// Snapshot retained scrollback plus the visible screen without changing viewport state.
    ///
    /// Returns logical lines oldest-to-newest: soft-wrapped grid rows are joined, wide-char
    /// spacer cells are skipped and combining marks follow their base character (see
    /// [`logical_lines_from_rows`]). The snapshot also carries the grid facts taken under the
    /// same lock (rows above the screen, scrollback capacity, columns) so a consumer can tell
    /// when no row can have been evicted (see [`HistorySnapshot`]). This reads the terminal's
    /// existing in-memory grid without cloning it; it does not capture keyboard input or
    /// write to disk.
    pub fn history_lines(&self) -> HistorySnapshot {
        let term = self.term.clone();
        let terminal = term.lock();
        grid_history_snapshot(&terminal, SCROLLBACK_LINES)
    }

    /// Scroll the viewport so a retained-history line is visible.
    ///
    /// `index` is a zero-based grid row counted from the oldest retained row (see
    /// [`LogicalLine::row_for_char`]).
    pub fn scroll_to_history_index(&mut self, index: usize) -> bool {
        let term = self.term.clone();
        let mut terminal = term.lock();
        if index >= terminal.total_lines() {
            return false;
        }

        let history_size = terminal.history_size();
        terminal.grid_mut().scroll_display(Scroll::Bottom);
        if index < history_size {
            let delta = history_size.saturating_sub(index).min(i32::MAX as usize) as i32;
            terminal.grid_mut().scroll_display(Scroll::Delta(delta));
        }
        true
    }

    fn process_link_action(
        &mut self,
        terminal: &Term<EventProxy>,
        link_action: LinkAction,
        point: Point,
    ) {
        match link_action {
            LinkAction::Hover => {
                self.last_content.hovered_hyperlink =
                    self.regex_match_at(terminal, point, &mut self.url_regex.clone());
            }
            LinkAction::Clear => {
                self.last_content.hovered_hyperlink = None;
            }
            LinkAction::Open => {
                self.open_link();
            }
        };
    }

    fn open_link(&self) {
        if let Some(range) = &self.last_content.hovered_hyperlink {
            let start = range.start();
            let end = range.end();

            let mut url = String::from(self.last_content.grid.index(*start).c);
            for indexed in self.last_content.grid.iter_from(*start) {
                url.push(indexed.c);
                if indexed.point == *end {
                    break;
                }
            }

            open::that(url).unwrap_or_else(|_| {
                panic!("link opening is failed");
            })
        }
    }

    fn process_mouse_report_flags(
        &self,
        button: MouseButton,
        shift: bool,
        alt: bool,
        command: bool,
        point: Point,
        pressed: bool,
    ) {
        let mut mods = 0;
        if shift {
            mods += 4;
        }
        if alt {
            mods += 8;
        }
        if command {
            mods += 16;
        }

        match MouseMode::from(self.last_content().terminal_mode) {
            MouseMode::Sgr => self.sgr_mouse_report(point, button as u8 + mods, pressed),
            MouseMode::Normal(is_utf8) => {
                if pressed {
                    self.normal_mouse_report(point, button as u8 + mods, is_utf8)
                } else {
                    self.normal_mouse_report(point, 3 + mods, is_utf8)
                }
            }
        }
    }

    fn sgr_mouse_report(&self, point: Point, button: u8, pressed: bool) {
        let c = if pressed { 'M' } else { 'm' };

        let msg = format!(
            "\x1b[<{};{};{}{}",
            button,
            point.column + 1,
            point.line + 1,
            c
        );

        self.notifier.notify(msg.as_bytes().to_vec());
    }

    fn normal_mouse_report(&self, point: Point, button: u8, is_utf8: bool) {
        let Point { line, column } = point;
        let max_point = if is_utf8 { 2015 } else { 223 };

        if line >= max_point || column >= max_point {
            return;
        }

        let mut msg = vec![b'\x1b', b'[', b'M', 32 + button];

        let mouse_pos_encode = |pos: usize| -> Vec<u8> {
            let pos = 32 + 1 + pos;
            let first = 0xC0 + pos / 64;
            let second = 0x80 + (pos & 63);
            vec![first as u8, second as u8]
        };

        if is_utf8 && column >= Column(95) {
            msg.append(&mut mouse_pos_encode(column.0));
        } else {
            msg.push(32 + 1 + column.0 as u8);
        }

        if is_utf8 && line >= 95 {
            msg.append(&mut mouse_pos_encode(line.0 as usize));
        } else {
            msg.push(32 + 1 + line.0 as u8);
        }

        self.notifier.notify(msg);
    }

    fn start_selection(
        &mut self,
        terminal: &mut Term<EventProxy>,
        selection_type: SelectionType,
        x: f32,
        y: f32,
    ) {
        start_drag_selection(
            terminal,
            &mut self.selection_anchor,
            &self.size,
            selection_type,
            x,
            y,
        );
    }

    fn update_selection(&mut self, terminal: &mut Term<EventProxy>, x: f32, y: f32) {
        update_drag_selection(terminal, &mut self.selection_anchor, &self.size, x, y);
    }

    fn resize(&mut self, terminal: &mut Term<EventProxy>, layout_size: Size, font_size: Size) {
        if layout_size == self.size.layout_size && font_size == self.size.cell_size {
            return;
        }

        if let Some(size) = TerminalSize::for_layout(layout_size, font_size) {
            self.size = size;

            self.notifier.on_resize(self.size.into());
            terminal.resize(TermSize::new(
                self.size.num_cols as usize,
                self.size.num_lines as usize,
            ));
        }
    }

    fn write<I: Into<Cow<'static, [u8]>>>(&self, input: I) {
        self.notifier.notify(input);
    }

    fn scroll(&mut self, terminal: &mut Term<EventProxy>, delta_value: i32) {
        if delta_value != 0 {
            let scroll = Scroll::Delta(delta_value);
            if terminal
                .mode()
                .contains(TermMode::ALTERNATE_SCROLL | TermMode::ALT_SCREEN)
            {
                let line_cmd = if delta_value > 0 { b'A' } else { b'B' };
                let mut content = vec![];

                for _ in 0..delta_value.abs() {
                    content.push(0x1b);
                    content.push(b'O');
                    content.push(line_cmd);
                }

                self.notifier.notify(content);
            } else {
                terminal.grid_mut().scroll_display(scroll);
            }
        }
    }

    /// Based on alacritty/src/display/hint.rs > regex_match_at
    /// Retrieve the match, if the specified point is inside the content matching the regex.
    fn regex_match_at(
        &self,
        terminal: &Term<EventProxy>,
        point: Point,
        regex: &mut RegexSearch,
    ) -> Option<Match> {
        let x = visible_regex_match_iter(terminal, regex).find(|rm| rm.contains(&point));
        x
    }
}

/// Copied from alacritty/src/display/hint.rs:
/// Iterate over all visible regex matches.
fn visible_regex_match_iter<'a>(
    term: &'a Term<EventProxy>,
    regex: &'a mut RegexSearch,
) -> impl Iterator<Item = Match> + 'a {
    let viewport_start = Line(-(term.grid().display_offset() as i32));
    let viewport_end = viewport_start + term.bottommost_line();
    let mut start = term.line_search_left(Point::new(viewport_start, Column(0)));
    let mut end = term.line_search_right(Point::new(viewport_end, Column(0)));
    start.line = start.line.max(viewport_start - 100);
    end.line = end.line.min(viewport_end + 100);

    RegexIter::new(start, end, Direction::Right, term, regex)
        .skip_while(move |rm| rm.end().line < viewport_start)
        .take_while(move |rm| rm.start().line <= viewport_end)
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct DisplayCell {
    pub character: char,
    pub row: usize,
    pub column: usize,
    pub foreground: [u8; 3],
    pub background: [u8; 3],
    pub cursor_color: [u8; 3],
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strikeout: bool,
    pub wide: bool,
    pub cursor: bool,
}

fn display_row_ranges(rows: usize, cells: &[DisplayCell]) -> Vec<(usize, usize)> {
    let mut row_ranges = vec![(usize::MAX, 0); rows];
    for (index, cell) in cells.iter().enumerate() {
        if let Some(range) = row_ranges.get_mut(cell.row) {
            if range.0 == usize::MAX {
                range.0 = index;
            }
            range.1 = index + 1;
        }
    }
    for range in &mut row_ranges {
        if range.0 == usize::MAX {
            *range = (0, 0);
        }
    }
    row_ranges
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DisplaySnapshot {
    pub rows: usize,
    pub columns: usize,
    pub background: [u8; 3],
    /// Contiguous cell index range for each visible row. Empty rows use (0, 0).
    pub row_ranges: Vec<(usize, usize)>,
    pub cells: Vec<DisplayCell>,
}

pub struct RenderableContent {
    pub grid: Grid<Cell>,
    pub hovered_hyperlink: Option<RangeInclusive<Point>>,
    pub selectable_range: Option<SelectionRange>,
    pub cursor: Cell,
    pub terminal_mode: TermMode,
    pub terminal_size: TerminalSize,
}

impl Default for RenderableContent {
    fn default() -> Self {
        Self {
            grid: Grid::new(0, 0, 0),
            hovered_hyperlink: None,
            selectable_range: None,
            cursor: Cell::default(),
            terminal_mode: TermMode::empty(),
            terminal_size: TerminalSize::default(),
        }
    }
}

impl Drop for TerminalBackend {
    fn drop(&mut self) {
        let _ = self.notifier.0.send(Msg::Shutdown);
        self.subscription_shutdown.store(true, Ordering::Release);
        if let Some(thread) = self.subscription_thread.take() {
            let _ = thread.join();
        }
    }
}

#[derive(Clone)]
pub struct EventProxy(mpsc::Sender<Event>);

impl EventListener for EventProxy {
    fn send_event(&self, event: Event) {
        let _ = self.0.send(event.clone());
    }
}

/// Side of the cell under `x` (text-area pixels): the half the pointer is in.
fn selection_side(size: &TerminalSize, x: f32) -> Side {
    let cell_x = x.max(0.0) % size.cell_size.width;

    if cell_x > size.cell_size.width / 2.0 {
        Side::Right
    } else {
        Side::Left
    }
}

/// First and last grid cell of the glyph at `point`: both halves of a wide
/// character, otherwise the cell itself.
fn glyph_cells<T>(terminal: &Term<T>, point: Point) -> (Point, Point) {
    let flags = terminal.grid()[point].flags;
    let last_column = terminal.columns().saturating_sub(1);
    if flags.contains(term::cell::Flags::WIDE_CHAR_SPACER) && point.column.0 > 0 {
        (Point::new(point.line, point.column - 1), point)
    } else if flags.contains(term::cell::Flags::WIDE_CHAR) && point.column.0 < last_column {
        (point, Point::new(point.line, point.column + 1))
    } else {
        (point, point)
    }
}

/// Horizontal distance (logical px, text-area coordinates) the pointer must
/// move from the press x, while still on the pressed glyph, before that one
/// glyph is selected. Below it a click with ordinary hand jitter selects and
/// copies nothing; at or above it a small drag selects a single character.
/// Vertical movement inside the cell does not count.
const SINGLE_GLYPH_DRAG_THRESHOLD_PX: f32 = 3.0;

/// Press position of a Simple drag selection: the text-area x it was pressed
/// at and which end of the live selection's range holds the pressed glyph.
/// The pressed grid cell itself is not stored: PTY output that scrolls the
/// grid makes Alacritty rotate `Term::selection`, so the cell is recovered
/// from the live selection on every move.
#[derive(Debug, Clone, Copy)]
struct DragAnchor {
    x: f32,
    /// The pressed glyph is the range end (the last build dragged backward).
    anchor_is_end: bool,
}

/// Pressed cell of the live Simple drag `selection`, as Alacritty has
/// rotated it with the content. The selection's own anchor is the pressed
/// glyph's first cell (`anchor_is_end` false) or last cell (true); a clone is
/// stretched to the far corner of the grid so the range is never empty.
fn live_drag_anchor<T>(
    terminal: &Term<T>,
    selection: &Selection,
    anchor_is_end: bool,
) -> Option<Point> {
    let mut probe = selection.clone();
    if anchor_is_end {
        probe.update(Point::new(terminal.topmost_line(), Column(0)), Side::Left);
        probe.to_range(terminal).map(|range| range.end)
    } else {
        let corner = Point::new(terminal.bottommost_line(), terminal.last_column());
        probe.update(corner, Side::Right);
        probe.to_range(terminal).map(|range| range.start)
    }
}

/// Simple drag selection from the glyph under `anchor` to the glyph under
/// `current`, both included whatever half of the cell the pointer is in.
/// While the pointer is still on the pressed glyph the selection holds that
/// one glyph only when `select_pressed_glyph` is set (the pointer has moved
/// far enough horizontally); otherwise it is empty, so a plain click
/// highlights and copies nothing.
fn inclusive_drag_selection<T>(
    terminal: &Term<T>,
    anchor: Point,
    current: Point,
    select_pressed_glyph: bool,
) -> Selection {
    let (anchor_first, anchor_last) = glyph_cells(terminal, anchor);
    let (current_first, current_last) = glyph_cells(terminal, current);
    // Alacritty's Simple range drops a start cell anchored on its Right side
    // and an end cell anchored on its Left side, so anchoring the earlier
    // glyph on Left and the later glyph on Right keeps both.
    let (start, start_side, end, end_side) = if current_first == anchor_first {
        if select_pressed_glyph {
            (anchor_first, Side::Left, anchor_last, Side::Right)
        } else {
            (anchor_first, Side::Left, anchor_first, Side::Left)
        }
    } else if current_first > anchor_first {
        (anchor_first, Side::Left, current_last, Side::Right)
    } else {
        (anchor_last, Side::Right, current_first, Side::Left)
    };
    let mut selection = Selection::new(SelectionType::Simple, start, start_side);
    selection.update(end, end_side);
    selection
}

fn start_drag_selection<T>(
    terminal: &mut Term<T>,
    anchor: &mut Option<DragAnchor>,
    size: &TerminalSize,
    selection_type: SelectionType,
    x: f32,
    y: f32,
) {
    let location = TerminalBackend::selection_point(x, y, size, terminal.grid().display_offset());
    if selection_type == SelectionType::Simple {
        *anchor = Some(DragAnchor {
            x,
            anchor_is_end: false,
        });
        terminal.selection = Some(inclusive_drag_selection(
            terminal, location, location, false,
        ));
    } else {
        *anchor = None;
        terminal.selection = Some(Selection::new(
            selection_type,
            location,
            selection_side(size, x),
        ));
    }
}

fn update_drag_selection<T>(
    terminal: &mut Term<T>,
    anchor: &mut Option<DragAnchor>,
    size: &TerminalSize,
    x: f32,
    y: f32,
) {
    let display_offset = terminal.grid().display_offset();
    let location = TerminalBackend::selection_point(x, y, size, display_offset);
    let simple = terminal
        .selection
        .as_ref()
        .is_some_and(|selection| selection.ty == SelectionType::Simple);
    match *anchor {
        Some(drag) if simple => {
            let pressed = terminal
                .selection
                .as_ref()
                .and_then(|selection| live_drag_anchor(terminal, selection, drag.anchor_is_end));
            let Some(pressed) = pressed else {
                *anchor = None;
                terminal.selection = None;
                return;
            };
            // Rebuilt from the pressed cell on every move: an existing
            // selection's anchor side cannot change once the drag reverses.
            let moved = (x - drag.x).abs() >= SINGLE_GLYPH_DRAG_THRESHOLD_PX;
            let anchor_first = glyph_cells(terminal, pressed).0;
            let current_first = glyph_cells(terminal, location).0;
            *anchor = Some(DragAnchor {
                x: drag.x,
                anchor_is_end: current_first < anchor_first,
            });
            terminal.selection = Some(inclusive_drag_selection(terminal, pressed, location, moved));
        }
        Some(_) if terminal.selection.is_none() => {
            // Alacritty dropped the selection (cleared, or scrolled out of
            // the history): do not resurrect it from a stale press cell.
            *anchor = None;
        }
        _ => {
            if let Some(ref mut selection) = terminal.selection {
                selection.update(location, selection_side(size, x));
            }
        }
    }
}

/// What a mouse drag selected: the copied text and the visible cells drawn
/// as selected, as `(row, column)` of each drawn glyph.
#[doc(hidden)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DragSelectionProbe {
    pub text: String,
    pub selected: Vec<(usize, usize)>,
}

/// Test seam: feed `lines` into a PTY-less `Term` of `size`, press at
/// `press` and move through `drags` (text-area pixels, i.e. after the GUI
/// inset) with a Simple selection, using the same selection code as
/// `BackendCommand::SelectStart`/`SelectUpdate`. The drawn cells use the same
/// `selectable_range` containment test as `display_snapshot`.
#[doc(hidden)]
pub fn probe_drag_selection(
    size: &TerminalSize,
    lines: &[&str],
    press: (f32, f32),
    drags: &[(f32, f32)],
) -> DragSelectionProbe {
    let steps: Vec<DragProbeStep<'_>> = drags
        .iter()
        .map(|&(x, y)| DragProbeStep::Move(x, y))
        .collect();
    probe_drag_selection_steps(size, lines, press, &steps)
}

/// One step of `probe_drag_selection_steps` while the button is held.
#[doc(hidden)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DragProbeStep<'a> {
    /// Pointer move to text-area pixels (x, y) with the button held.
    Move(f32, f32),
    /// Bytes the PTY writes while the button is held.
    Feed(&'a str),
}

/// Test seam: like `probe_drag_selection`, but PTY output can be fed
/// between pointer moves while the button is held.
#[doc(hidden)]
pub fn probe_drag_selection_steps(
    size: &TerminalSize,
    lines: &[&str],
    press: (f32, f32),
    steps: &[DragProbeStep<'_>],
) -> DragSelectionProbe {
    use alacritty_terminal::event::VoidListener;
    use alacritty_terminal::vte::ansi::{Processor, StdSyncHandler};

    let dimensions = TermSize::new(size.num_cols as usize, size.num_lines as usize);
    let mut terminal = Term::new(term::Config::default(), &dimensions, VoidListener);
    let mut parser: Processor<StdSyncHandler> = Processor::new();
    parser.advance(&mut terminal, lines.join("\r\n").as_bytes());

    let mut anchor = None;
    start_drag_selection(
        &mut terminal,
        &mut anchor,
        size,
        SelectionType::Simple,
        press.0,
        press.1,
    );
    for step in steps {
        match *step {
            DragProbeStep::Move(x, y) => {
                update_drag_selection(&mut terminal, &mut anchor, size, x, y);
            }
            DragProbeStep::Feed(bytes) => parser.advance(&mut terminal, bytes.as_bytes()),
        }
    }

    let range = terminal
        .selection
        .as_ref()
        .and_then(|selection| selection.to_range(&terminal));
    let grid = terminal.grid();
    let selected = grid
        .display_iter()
        .filter(|indexed| {
            !indexed
                .cell
                .flags
                .contains(term::cell::Flags::WIDE_CHAR_SPACER)
        })
        .filter(|indexed| range.is_some_and(|range| range.contains(indexed.point)))
        .map(|indexed| {
            (
                (indexed.point.line.0 + grid.display_offset() as i32) as usize,
                indexed.point.column.0,
            )
        })
        .collect();
    DragSelectionProbe {
        text: terminal.selection_to_string().unwrap_or_default(),
        selected,
    }
}

/// Grid rows of scrollback kept per pane (above the screen).
///
/// Memory: alacritty cells are 24 bytes and every row is allocated at full width, so at 150
/// columns a line costs about 3.6 KB and a full pane about 36 MB (10,000 x 3,632 B).
/// alacritty grows history lazily, so a pane only pays for the rows it has actually used.
/// Raising it (for example to 50,000, about 182 MB per full pane) waits for release-build
/// latency and peak-RSS measurements at both sizes.
pub const SCROLLBACK_LINES: usize = 10_000;

/// Retained history lines plus the grid facts needed to keep line identity across snapshots.
///
/// alacritty exposes no eviction count: once `history_rows == capacity` the history size is
/// constant while rows keep rotating out at the top. Below capacity no row can be evicted,
/// so rows `0..history_rows` of two snapshots with the same `columns` are the same rows as
/// long as history did not shrink (it shrinks on clear, reset or a screen-height increase).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HistorySnapshot {
    /// Logical lines oldest-to-newest: scrollback, then the screen.
    pub lines: Vec<LogicalLine>,
    /// Grid rows above the screen when the snapshot was taken. Rows `0..history_rows` are
    /// immutable scrollback; later rows are the live screen and may be rewritten in place.
    pub history_rows: usize,
    /// Most scrollback rows the active grid keeps: [`SCROLLBACK_LINES`] on the main screen,
    /// `0` on the alternate screen (alacritty gives it no scrollback). A change means the
    /// snapshot came from the other grid.
    pub capacity: usize,
    /// Grid columns; a change means every row was reflowed.
    pub columns: usize,
}

/// One logical line of retained history: soft-wrapped grid rows joined into one string.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LogicalLine {
    /// Text with wide-char spacers removed and combining marks after their base character.
    /// Trailing spaces of the last row are trimmed.
    pub text: String,
    /// Zero-based grid row (from the oldest retained row) of the first row of this line.
    pub first_row: usize,
    /// Char offsets into `text` at which each grid row starts; always begins with `0`.
    pub segment_starts: Vec<usize>,
}

impl LogicalLine {
    /// Grid row containing the char at `char_offset` (zero-based) of `text`.
    pub fn row_for_char(&self, char_offset: usize) -> usize {
        let segment = self
            .segment_starts
            .iter()
            .rposition(|start| *start <= char_offset)
            .unwrap_or(0);
        self.first_row + segment
    }
}

/// One grid cell as seen by [`logical_lines_from_rows`].
#[derive(Clone, Copy, Debug)]
pub struct HistoryCell<'a> {
    pub c: char,
    /// `WIDE_CHAR_SPACER` or `LEADING_WIDE_CHAR_SPACER`: not part of the text.
    pub spacer: bool,
    /// Zero-width (combining) characters attached to this cell.
    pub zerowidth: &'a [char],
}

/// Build logical lines from grid rows, oldest first. Each row is its cells plus whether it
/// soft-wraps into the next row (`WRAPLINE` on its last cell). A row that exactly fills the
/// width and then ends with a newline is not wrapped and is not joined.
pub fn logical_lines_from_rows<'a, R, C>(rows: R) -> Vec<LogicalLine>
where
    R: IntoIterator<Item = (C, bool)>,
    C: IntoIterator<Item = HistoryCell<'a>>,
{
    let mut lines = Vec::new();
    let mut current = LogicalLine::default();
    let mut chars = 0usize;
    let mut open = false;
    for (row_index, (cells, wrapped)) in rows.into_iter().enumerate() {
        if !open {
            current = LogicalLine {
                text: String::new(),
                first_row: row_index,
                segment_starts: Vec::new(),
            };
            chars = 0;
            open = true;
        }
        current.segment_starts.push(chars);
        for cell in cells {
            if cell.spacer {
                continue;
            }
            current.text.push(cell.c);
            chars += 1;
            for mark in cell.zerowidth {
                current.text.push(*mark);
                chars += 1;
            }
        }
        if !wrapped {
            let trimmed = current.text.trim_end_matches(' ').len();
            current.text.truncate(trimmed);
            lines.push(std::mem::take(&mut current));
            open = false;
        }
    }
    if open {
        let trimmed = current.text.trim_end_matches(' ').len();
        current.text.truncate(trimmed);
        lines.push(current);
    }
    lines
}

/// [`HistorySnapshot`] of the active alacritty grid; `capacity` is the main grid's
/// configured `scrolling_history` (the alternate grid keeps none).
fn grid_history_snapshot<T>(terminal: &Term<T>, capacity: usize) -> HistorySnapshot {
    let alternate = terminal.mode().contains(TermMode::ALT_SCREEN);
    HistorySnapshot {
        lines: grid_logical_lines(terminal),
        history_rows: terminal.history_size(),
        capacity: if alternate { 0 } else { capacity },
        columns: terminal.columns(),
    }
}

/// Adapt a real alacritty grid (topmost retained row to bottom of screen) to
/// [`logical_lines_from_rows`].
fn grid_logical_lines<T>(terminal: &Term<T>) -> Vec<LogicalLine> {
    let grid = terminal.grid();
    let top = terminal.topmost_line().0;
    let bottom = terminal.bottommost_line().0;
    let spacer = term::cell::Flags::WIDE_CHAR_SPACER | term::cell::Flags::LEADING_WIDE_CHAR_SPACER;
    logical_lines_from_rows((top..=bottom).map(|line| {
        let row = &grid[Line(line)];
        let columns = row.len();
        let wrapped = columns > 0
            && row[Column(columns - 1)]
                .flags
                .contains(term::cell::Flags::WRAPLINE);
        let cells = (0..columns).map(move |column| {
            let cell = &row[Column(column)];
            HistoryCell {
                c: cell.c,
                spacer: cell.flags.intersects(spacer),
                zerowidth: cell.zerowidth().unwrap_or(&[]),
            }
        });
        (cells, wrapped)
    }))
}

#[cfg(test)]
mod logical_line_tests {
    use super::*;

    fn row(text: &str, wrapped: bool) -> (Vec<HistoryCell<'static>>, bool) {
        let cells = text
            .chars()
            .map(|c| HistoryCell {
                c,
                spacer: false,
                zerowidth: &[],
            })
            .collect();
        (cells, wrapped)
    }

    #[test]
    fn wide_spacers_are_skipped() {
        let cells = vec![
            HistoryCell {
                c: '東',
                spacer: false,
                zerowidth: &[],
            },
            HistoryCell {
                c: ' ',
                spacer: true,
                zerowidth: &[],
            },
            HistoryCell {
                c: '京',
                spacer: false,
                zerowidth: &[],
            },
            HistoryCell {
                c: ' ',
                spacer: true,
                zerowidth: &[],
            },
        ];
        let lines = logical_lines_from_rows(vec![(cells, false)]);
        assert_eq!(lines[0].text, "東京");
    }

    #[test]
    fn combining_marks_follow_their_base() {
        static ACUTE: [char; 1] = ['\u{301}'];
        let cells = vec![
            HistoryCell {
                c: 'e',
                spacer: false,
                zerowidth: &ACUTE,
            },
            HistoryCell {
                c: 'x',
                spacer: false,
                zerowidth: &[],
            },
        ];
        let lines = logical_lines_from_rows(vec![(cells, false)]);
        assert_eq!(lines[0].text, "e\u{301}x");
    }

    #[test]
    fn wrapped_rows_join_and_map_offsets_to_rows() {
        let lines = logical_lines_from_rows(vec![
            row("top   ", false),
            row("abcdef", true),
            row("ghi   ", false),
        ]);
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].text, "top");
        assert_eq!(lines[1].text, "abcdefghi");
        assert_eq!(lines[1].first_row, 1);
        assert_eq!(lines[1].segment_starts, vec![0, 6]);
        // "efg" straddles the wrap and starts on the first row; "ghi" starts on the second.
        assert_eq!(lines[1].row_for_char(lines[1].text.find("efg").unwrap()), 1);
        assert_eq!(lines[1].row_for_char(lines[1].text.find("ghi").unwrap()), 2);
    }

    #[test]
    fn full_width_row_without_wrap_flag_is_not_joined() {
        let lines = logical_lines_from_rows(vec![row("abcdef", false), row("ghi", false)]);
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].text, "abcdef");
        assert_eq!(lines[1].first_row, 1);
    }

    #[test]
    fn real_term_cjk_wraps_into_one_logical_line() {
        use alacritty_terminal::event::VoidListener;
        use alacritty_terminal::vte::ansi::{Processor, StdSyncHandler};

        let dimensions = TermSize::new(10, 4);
        let mut terminal = Term::new(term::Config::default(), &dimensions, VoidListener);
        let mut parser: Processor<StdSyncHandler> = Processor::new();
        // 東京 takes 4 columns; 10 ASCII chars force one soft wrap at width 10.
        parser.advance(&mut terminal, "東京abcdefghij\r\nnext".as_bytes());
        let lines = grid_logical_lines(&terminal);
        assert_eq!(lines[0].text, "東京abcdefghij");
        assert_eq!(lines[0].first_row, 0);
        // Row 1 starts after 東京abcdef (8 chars, 10 columns).
        assert_eq!(lines[0].segment_starts, vec![0, 8]);
        assert_eq!(lines[1].text, "next");
        assert_eq!(lines[1].first_row, 2);
    }

    #[test]
    fn real_term_snapshot_reports_history_rows_capacity_and_columns() {
        use alacritty_terminal::event::VoidListener;
        use alacritty_terminal::vte::ansi::{Processor, StdSyncHandler};

        let dimensions = TermSize::new(10, 4);
        let config = term::Config {
            scrolling_history: 3,
            ..term::Config::default()
        };
        let mut terminal = Term::new(config, &dimensions, VoidListener);
        let mut parser: Processor<StdSyncHandler> = Processor::new();
        // Seven rows on a four-row screen: three rows scroll into history.
        parser.advance(&mut terminal, b"r0\r\nr1\r\nr2\r\nr3\r\nr4\r\nr5\r\nr6");
        let snapshot = grid_history_snapshot(&terminal, 3);
        assert_eq!(
            (snapshot.history_rows, snapshot.capacity, snapshot.columns),
            (3, 3, 10)
        );
        assert_eq!(snapshot.lines.len(), 7);
        assert_eq!(snapshot.lines[0].text, "r0");
        assert_eq!(snapshot.lines[3].first_row, 3);

        // At capacity the history size stays constant while the oldest rows rotate out.
        parser.advance(&mut terminal, b"\r\nr7\r\nr8");
        let snapshot = grid_history_snapshot(&terminal, 3);
        assert_eq!(snapshot.history_rows, 3);
        assert_eq!(snapshot.lines.len(), 7);
        assert_eq!(snapshot.lines[0].text, "r2");
        assert_eq!(snapshot.lines[6].text, "r8");

        // The alternate screen has no scrollback; leaving it restores the main grid.
        parser.advance(&mut terminal, b"\x1b[?1049hvim");
        let snapshot = grid_history_snapshot(&terminal, 3);
        assert_eq!(
            (snapshot.history_rows, snapshot.capacity, snapshot.columns),
            (0, 0, 10)
        );
        parser.advance(&mut terminal, b"\x1b[?1049l");
        let snapshot = grid_history_snapshot(&terminal, 3);
        assert_eq!((snapshot.history_rows, snapshot.capacity), (3, 3));
        assert_eq!(snapshot.lines[0].text, "r2");
    }
}

#[cfg(test)]
mod display_snapshot_tests {
    use super::*;

    fn cell(row: usize, column: usize, character: char) -> DisplayCell {
        DisplayCell {
            character,
            row,
            column,
            foreground: [255, 255, 255],
            background: [0, 0, 0],
            cursor_color: [255, 255, 255],
            bold: false,
            italic: false,
            underline: false,
            strikeout: false,
            wide: false,
            cursor: false,
        }
    }

    #[test]
    fn row_ranges_cover_contiguous_visible_cells_and_empty_rows() {
        let cells = vec![
            cell(0, 0, 'a'),
            cell(0, 1, 'b'),
            cell(2, 0, 'c'),
            cell(2, 1, 'd'),
            cell(2, 2, 'e'),
        ];

        assert_eq!(
            display_row_ranges(4, &cells),
            vec![(0, 2), (0, 0), (2, 5), (0, 0)]
        );
    }

    #[test]
    fn row_ranges_ignore_cells_outside_visible_row_count() {
        let cells = vec![cell(0, 0, 'a'), cell(4, 0, 'x')];
        assert_eq!(display_row_ranges(2, &cells), vec![(0, 1), (0, 0)]);
    }
}
