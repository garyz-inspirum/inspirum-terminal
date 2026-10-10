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
}

impl Default for TerminalSize {
    fn default() -> Self {
        Self {
            cell_width: 1,
            cell_height: 1,
            num_cols: 80,
            num_lines: 50,
            layout_size: Size::default(),
        }
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
            scrolling_history: 10_000,
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
        let col = (x as usize) / (terminal_size.cell_width as usize);
        let col = min(Column(col), Column(terminal_size.num_cols as usize - 1));

        let line = (y as usize) / (terminal_size.cell_height as usize);
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
    /// Lines are returned oldest-to-newest and trailing blank cells are trimmed. This reads the
    /// terminal's existing in-memory grid; it does not capture keyboard input or write to disk.
    pub fn history_lines(&self) -> Vec<String> {
        let term = self.term.clone();
        let terminal = term.lock();
        let grid = terminal.grid();
        let start = Point::new(terminal.topmost_line(), Column(0));
        let end_line = terminal.bottommost_line();

        let mut lines = Vec::with_capacity(terminal.total_lines());
        // Grid::iter_from advances from the supplied point, so seed the first retained cell.
        let mut current_line = Some(start.line);
        let mut current = String::from(grid.index(start).c);
        for indexed in grid.iter_from(start) {
            if indexed.point.line > end_line {
                break;
            }
            if current_line != Some(indexed.point.line) {
                lines.push(current.trim_end_matches(' ').to_owned());
                current.clear();
                current_line = Some(indexed.point.line);
            }
            current.push(indexed.c);
        }
        lines.push(current.trim_end_matches(' ').to_owned());
        lines
    }

    /// Scroll the viewport so a retained-history line is visible.
    ///
    /// `index` is zero-based from the oldest retained line returned by `history_lines`.
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
        let location = Self::selection_point(x, y, &self.size, terminal.grid().display_offset());
        terminal.selection = Some(Selection::new(
            selection_type,
            location,
            self.selection_side(x),
        ));
    }

    fn update_selection(&mut self, terminal: &mut Term<EventProxy>, x: f32, y: f32) {
        let display_offset = terminal.grid().display_offset();
        if let Some(ref mut selection) = terminal.selection {
            let location = Self::selection_point(x, y, &self.size, display_offset);
            selection.update(location, self.selection_side(x));
        }
    }

    fn selection_side(&self, x: f32) -> Side {
        let cell_x = x as usize % self.size.cell_width as usize;
        let half_cell_width = (self.size.cell_width as f32 / 2.0) as usize;

        if cell_x > half_cell_width {
            Side::Right
        } else {
            Side::Left
        }
    }

    fn resize(&mut self, terminal: &mut Term<EventProxy>, layout_size: Size, font_size: Size) {
        if layout_size == self.size.layout_size
            && font_size.width as u16 == self.size.cell_width
            && font_size.height as u16 == self.size.cell_height
        {
            return;
        }

        let lines = (layout_size.height / font_size.height.floor()) as u16;
        let cols = (layout_size.width / font_size.width.floor()) as u16;
        if lines > 0 && cols > 0 {
            self.size = TerminalSize {
                layout_size,
                cell_height: font_size.height as u16,
                cell_width: font_size.width as u16,
                num_lines: lines,
                num_cols: cols,
            };

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
