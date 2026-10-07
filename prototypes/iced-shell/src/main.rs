//! Native Iced interaction prototype. Every terminal/file is a labelled fixture.
//! This executable neither reads production profiles nor starts SSH/PTY processes.
mod model;
use iced::widget::{button, center, column, container, mouse_area, opaque, operation, pane_grid, row, scrollable, space, stack, text, text_input};
use iced::{event, keyboard, Border, Color, Element, Fill, Font, Subscription, Task, Theme};
use model::{ConnectionForm, Profile};

const BG: Color = Color::from_rgb8(14, 18, 25);
const PANEL: Color = Color::from_rgb8(22, 29, 40);
const ELEVATED: Color = Color::from_rgb8(30, 40, 55);
const LINE: Color = Color::from_rgb8(48, 62, 81);
const FG: Color = Color::from_rgb8(231, 237, 247);
const MUTED: Color = Color::from_rgb8(163, 177, 196);
const BLUE: Color = Color::from_rgb8(123, 176, 255);
const GREEN: Color = Color::from_rgb8(113, 217, 171);

fn main() -> iced::Result {
    iced::application(App::boot, App::update, App::view)
        .title("Inspirum - Iced design preview")
        .theme(|_: &App| Theme::custom("Inspirum".into(), iced::theme::Palette {
            background: BG, text: FG, primary: BLUE, success: GREEN,
            danger: Color::from_rgb8(255, 138, 151),
        }))
        .default_text_size(14)
        .window(iced::window::Settings {
            size: iced::Size::new(1280.0, 800.0),
            min_size: Some(iced::Size::new(960.0, 640.0)),
            ..Default::default()
        })
        .scale_factor(|app: &App| app.scale)
        .subscription(|_: &App| event::listen().map(Message::Event))
        .run()
}

struct Workspace {
    profile: Profile,
    panes: pane_grid::State<Profile>,
    focus: pane_grid::Pane,
}
impl Workspace {
    fn new(profile: Profile) -> Self {
        let (panes, focus) = pane_grid::State::new(profile.clone());
        Self { profile, panes, focus }
    }
    fn split(&mut self, axis: pane_grid::Axis) {
        if self.panes.len() < 4 {
            if let Some((pane, _)) = self.panes.split(axis, self.focus, self.profile.clone()) {
                self.focus = pane;
            }
        }
    }
}
#[derive(Clone, Copy)]
enum Dock { Terminal, Files }
#[derive(Clone)]
enum Dialog { Connection, Commands, Close(usize), About }
struct App {
    profiles: Vec<Profile>,
    query: String,
    tabs: Vec<Workspace>,
    active: usize,
    dock: pane_grid::State<Dock>,
    terminal_dock: pane_grid::Pane,
    files_dock: Option<pane_grid::Pane>,
    form: ConnectionForm,
    dialog: Option<Dialog>,
    selected_file: Option<usize>,
    preview_transfers: Vec<String>,
    scale: f64,
}
#[derive(Clone, Debug)]
enum Message {
    Search(String), Open(usize), SelectTab(usize), New, Commands, About,
    CloseDialog, AskClose(usize), ConfirmClose(usize), ToggleFiles,
    Split(pane_grid::Axis), Focus(pane_grid::Pane), Resize(pane_grid::ResizeEvent),
    Drag(pane_grid::DragEvent), ClosePane(pane_grid::Pane), DockResize(pane_grid::ResizeEvent),
    Name(String), Host(String), User(String), Port(String), Advanced, Submit,
    File(usize), QueuePreview, Scale(f64), Event(iced::Event),
}

impl App {
    fn empty() -> Self {
        let (dock, terminal_dock) = pane_grid::State::new(Dock::Terminal);
        Self { profiles: model::samples(), query: String::new(), tabs: Vec::new(), active: 0,
            dock, terminal_dock, files_dock: None, form: ConnectionForm::default(), dialog: None,
            selected_file: None, preview_transfers: Vec::new(), scale: 1.0 }
    }
    fn boot() -> Self {
        let mut app = Self::empty();
        let mode = std::env::args().find_map(|arg| arg.strip_prefix("--preview=").map(str::to_owned)).unwrap_or_default();
        if ["terminal", "split", "files", "dialog"].contains(&mode.as_str()) {
            app.open(0);
        }
        if mode == "split" { app.tabs[0].split(pane_grid::Axis::Vertical); }
        if mode == "files" { app.toggle_files(); }
        if mode == "dialog" { app.dialog = Some(Dialog::Connection); }
        app
    }
    fn open(&mut self, index: usize) {
        if let Some(profile) = self.profiles.get(index).cloned() {
            if let Some(existing) = self.tabs.iter().position(|tab| tab.profile == profile) {
                self.active = existing;
            } else {
                self.tabs.push(Workspace::new(profile));
                self.active = self.tabs.len() - 1;
            }
        }
        self.dialog = None;
        self.selected_file = None;
    }
    fn toggle_files(&mut self) {
        if let Some(pane) = self.files_dock.take() {
            self.dock.close(pane);
        } else if !self.tabs.is_empty() {
            if let Some((pane, split)) = self.dock.split(pane_grid::Axis::Horizontal, self.terminal_dock, Dock::Files) {
                self.files_dock = Some(pane);
                self.dock.resize(split, 0.60);
            }
        }
    }
    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Search(value) => self.query = value,
            Message::Open(index) => self.open(index),
            Message::SelectTab(index) => { if index < self.tabs.len() { self.active = index; self.selected_file = None; } }
            Message::New => { self.form = ConnectionForm::default(); self.dialog = Some(Dialog::Connection); return operation::focus_next(); }
            Message::Commands => self.dialog = Some(Dialog::Commands),
            Message::About => self.dialog = Some(Dialog::About),
            Message::CloseDialog => { self.dialog = None; self.form = ConnectionForm::default(); }
            Message::AskClose(index) => self.dialog = Some(Dialog::Close(index)),
            Message::ConfirmClose(index) => {
                if index < self.tabs.len() {
                    self.tabs.remove(index);
                    if index < self.active { self.active -= 1; }
                    self.active = self.active.min(self.tabs.len().saturating_sub(1));
                }
                if self.tabs.is_empty() && self.files_dock.is_some() { self.toggle_files(); }
                self.dialog = None;
                self.selected_file = None;
            }
            Message::ToggleFiles => { self.dialog = None; self.toggle_files(); }
            Message::Split(axis) => { self.dialog = None; if let Some(tab) = self.tabs.get_mut(self.active) { tab.split(axis); } }
            Message::Focus(pane) => { if let Some(tab) = self.tabs.get_mut(self.active) { tab.focus = pane; } }
            Message::Resize(event) => { if let Some(tab) = self.tabs.get_mut(self.active) { tab.panes.resize(event.split, event.ratio.clamp(0.18, 0.82)); } }
            Message::Drag(pane_grid::DragEvent::Dropped { pane, target }) => { if let Some(tab) = self.tabs.get_mut(self.active) { tab.panes.drop(pane, target); } }
            Message::Drag(_) => {}
            Message::ClosePane(pane) => {
                if let Some(tab) = self.tabs.get_mut(self.active) {
                    if tab.panes.len() == 1 { self.dialog = Some(Dialog::Close(self.active)); }
                    else if let Some((_, sibling)) = tab.panes.close(pane) { tab.focus = sibling; }
                }
            }
            Message::DockResize(event) => self.dock.resize(event.split, event.ratio.clamp(0.3, 0.75)),
            Message::Name(value) => self.form.name = value,
            Message::Host(value) => self.form.host = value,
            Message::User(value) => self.form.user = value,
            Message::Port(value) => self.form.port = value,
            Message::Advanced => self.form.advanced = !self.form.advanced,
            Message::Submit => match self.form.validate() {
                Ok(profile) => { self.profiles.push(profile); self.open(self.profiles.len() - 1); self.form = ConnectionForm::default(); }
                Err(error) => self.form.error = Some(error),
            },
            Message::File(index) => self.selected_file = Some(index),
            Message::QueuePreview => {
                if let (Some(index), Some(tab)) = (self.selected_file, self.tabs.get(self.active)) {
                    let files = ["deployment.yml", "notes.txt", "service.log"];
                    if let Some(file) = files.get(index) {
                        self.preview_transfers.push(format!("{}  /  {}  /  Simulated only", tab.profile.name, file));
                    }
                }
            }
            Message::Scale(delta) => self.scale = (self.scale + delta).clamp(0.85, 1.50),
            Message::Event(iced::Event::Keyboard(keyboard::Event::KeyPressed { key, modifiers, .. })) => {
                use keyboard::key::Named;
                match key.as_ref() {
                    keyboard::Key::Named(Named::Escape) => return self.update(Message::CloseDialog),
                    keyboard::Key::Named(Named::Tab) => return if modifiers.shift() { operation::focus_previous() } else { operation::focus_next() },
                    keyboard::Key::Character("n") if modifiers.command() && self.dialog.is_none() => return self.update(Message::New),
                    keyboard::Key::Character("p") if modifiers.command() && modifiers.shift() && self.dialog.is_none() => return self.update(Message::Commands),
                    _ => {}
                }
            }
            Message::Event(_) => {}
        }
        Task::none()
    }
    fn view(&self) -> Element<'_, Message> {
        let header = container(row![
            container(text(">_").size(21).color(BLUE)).padding([3, 8]).style(card),
            text("Inspirum").size(19), text("/  Terminal workspace").color(MUTED),
            space::horizontal(),
            action("Commands", Message::Commands),
            action("A-", Message::Scale(-0.1)), action("A+", Message::Scale(0.1)),
            action("Design preview", Message::About),
        ].spacing(12).align_y(iced::Center)).padding([10, 16]).style(surface);
        let footer = container(row![
            text("PREVIEW").size(12).color(BLUE),
            text("No network connections. No files changed.").size(12).color(MUTED),
            space::horizontal(), text(format!("{} sample sessions  /  Iced 0.14", self.tabs.len())).size(12).color(MUTED),
        ].spacing(12).align_y(iced::Center)).padding([9, 16]).style(surface);
        let base = column![header, row![self.sidebar(), self.workspace()].height(Fill), footer].height(Fill);
        if let Some(dialog) = &self.dialog {
            let content = self.dialog_view(dialog);
            stack![base, opaque(mouse_area(center(opaque(content)).style(|_| container::Style {
                background: Some(Color::from_rgba(0.02, 0.03, 0.05, 0.82).into()), ..Default::default()
            })).on_press(Message::CloseDialog))].into()
        } else { base.into() }
    }
    fn sidebar(&self) -> Element<'_, Message> {
        let mut list = column![
            row![text("SESSIONS").size(12).color(MUTED), space::horizontal(), text(self.profiles.len().to_string()).size(12).color(MUTED)],
            action("+  New connection", Message::New).style(primary).width(Fill),
            text_input("Search sessions...", &self.query).on_input(Message::Search).padding(10),
            space::vertical().height(6),
        ].spacing(12);
        let mut group = String::new();
        let mut count = 0;
        for (index, profile) in self.profiles.iter().enumerate().filter(|(_, p)| model::matches(p, &self.query)) {
            if group != profile.group {
                list = list.push(container(text(profile.group.clone()).size(12).color(MUTED)).padding([8, 2]));
                group = profile.group.clone();
            }
            let selected = self.tabs.get(self.active).map(|t| &t.profile) == Some(profile);
            let item = button(column![
                row![text(profile.name.clone()).size(14), space::horizontal(), text("SSH").size(11).color(BLUE)].spacing(4),
                text(format!("{}@{}", profile.user, profile.host)).size(12).color(MUTED),
            ].spacing(5)).padding(10).width(Fill).on_press(Message::Open(index))
                .style(if selected { selected_button } else { quiet });
            list = list.push(item);
            count += 1;
        }
        if count == 0 { list = list.push(text("No matching sessions").color(MUTED)); }
        container(column![scrollable(list).height(Fill), text("Sample profiles only\nClick a session to preview its workspace.").size(12).color(MUTED)].spacing(16))
            .width(256).height(Fill).padding(16).style(surface).into()
    }
    fn workspace(&self) -> Element<'_, Message> {
        if self.tabs.is_empty() {
            return center(column![
                text(">_").size(56).color(BLUE),
                text("Your workspace, ready.").size(30),
                text("Open a saved session or create a new connection.").size(16).color(MUTED),
                space::vertical().height(12),
                row![action("+  New connection", Message::New).style(primary), action("Explore a sample", Message::Open(0))].spacing(12),
                space::vertical().height(12),
                text("Connect. Split. Transfer. Stay focused.").size(14).color(MUTED),
            ].spacing(12).align_x(iced::Center)).into();
        }
        let mut tabs = row![].spacing(4);
        for (index, tab) in self.tabs.iter().enumerate() {
            tabs = tabs.push(container(row![
                action(tab.profile.name.clone(), Message::SelectTab(index)).style(if index == self.active { selected_button } else { quiet }),
                action("x", Message::AskClose(index)),
            ].spacing(0)).style(if index == self.active { active_card } else { surface }));
        }
        tabs = tabs.push(action("+", Message::New));
        let toolbar = row![
            text("SSH PREVIEW").size(12).color(GREEN), space::horizontal(),
            action("Split right", Message::Split(pane_grid::Axis::Vertical)),
            action("Split down", Message::Split(pane_grid::Axis::Horizontal)),
            action(if self.files_dock.is_some() { "Hide files" } else { "Files" }, Message::ToggleFiles),
        ].spacing(8).align_y(iced::Center);
        let dock = pane_grid(&self.dock, |_, panel, _| {
            pane_grid::Content::new(match panel { Dock::Terminal => self.terminals(), Dock::Files => self.files() })
        }).spacing(6).on_resize(8, Message::DockResize).height(Fill);
        container(column![scrollable(tabs).direction(scrollable::Direction::Horizontal(scrollable::Scrollbar::default())), toolbar, dock].spacing(10))
            .padding(14).height(Fill).width(Fill).into()
    }
    fn terminals(&self) -> Element<'_, Message> {
        let tab = &self.tabs[self.active];
        pane_grid(&tab.panes, |id, profile, _| {
            let focused = tab.focus == id;
            let title = pane_grid::TitleBar::new(row![
                text(if focused { "FOCUSED" } else { "SSH" }).size(11).color(if focused { BLUE } else { MUTED }),
                text(format!("{}@{}", profile.user, profile.name)).size(13),
                space::horizontal(), action("x", Message::ClosePane(id)),
            ].spacing(10).align_y(iced::Center)).padding([4, 10]).style(surface);
            let transcript = format!("{}@{}:~$ pwd\n/home/{}\n\n{}@{}:~$ ls -lh\ntotal 24K\ndrwxr-xr-x  4 ops ops  4.0K  config/\ndrwxr-xr-x  2 ops ops  4.0K  logs/\n-rw-r--r--  1 ops ops   892  deployment.yml\n\n{}@{}:~$ ", profile.user, profile.name, profile.user, profile.user, profile.name, profile.user, profile.name);
            let body = column![
                text("SIMULATED TERMINAL / sample output, not a live shell").size(12).color(MUTED),
                text(transcript).font(Font::MONOSPACE).size(15).color(FG),
                space::vertical(),
                text("Drag the pane header to rearrange. Drag dividers to resize.").size(12).color(MUTED),
            ].spacing(18).height(Fill);
            pane_grid::Content::new(container(scrollable(body)).padding(18).height(Fill))
                .title_bar(title).style(if focused { active_card } else { card })
        }).spacing(8).on_click(Message::Focus).on_drag(Message::Drag).on_resize(8, Message::Resize).height(Fill).into()
    }
    fn files(&self) -> Element<'_, Message> {
        let tab = &self.tabs[self.active];
        let remote = format!("REMOTE / {}", tab.profile.name);
        let mut remote_rows = column![text(remote).size(12).color(BLUE), text("/home/ops  /  sample files").size(13).color(MUTED)].spacing(8);
        for (index, (name, size)) in [("deployment.yml", "892 B"), ("notes.txt", "1.2 KB"), ("service.log", "12.4 KB")].iter().enumerate() {
            remote_rows = remote_rows.push(button(row![text(*name), space::horizontal(), text(*size).size(12).color(MUTED)])
                .padding(8).width(Fill).on_press(Message::File(index))
                .style(if self.selected_file == Some(index) { selected_button } else { quiet }));
        }
        let local = column![text("LOCAL / sample computer").size(12).color(MUTED), text("~/Downloads").size(13).color(MUTED),
            container(text("Select a remote file, then preview a transfer.\nNo real files are read or written.").color(MUTED)).padding([18, 0])].spacing(8);
        let queue = if self.preview_transfers.is_empty() { "Transfer queue is empty".into() } else { self.preview_transfers.join("\n") };
        let transfer = button(text("Queue preview transfer")).padding([7, 12]).style(quiet)
            .on_press_maybe(self.selected_file.map(|_| Message::QueuePreview));
        container(scrollable(column![
            row![text("Files").size(16), text("SFTP design preview").size(12).color(MUTED), space::horizontal(), transfer].spacing(12).align_y(iced::Center),
            row![container(local).width(Fill).padding(10), container(remote_rows).width(Fill).padding(10)].spacing(12),
            text(format!("TRANSFERS ({})", self.preview_transfers.len())).size(12).color(MUTED),
            text(queue).size(12).color(MUTED),
        ].spacing(10))).padding(14).height(Fill).style(card).into()
    }
    fn dialog_view(&self, dialog: &Dialog) -> Element<'_, Message> {
        let body: Element<'_, Message> = match dialog {
            Dialog::Connection => {
                let mut form = column![
                    text("New SSH connection").size(24),
                    text("Keep the essentials simple. Advanced options stay out of the way.").color(MUTED),
                    field("Session name", "Optional; defaults to host", &self.form.name, Message::Name),
                    field("Host", "server.example or IP address", &self.form.host, Message::Host),
                    row![field("Username", "Default SSH user", &self.form.user, Message::User), container(field("Port", "22", &self.form.port, Message::Port)).width(120)].spacing(14),
                    action(if self.form.advanced { "-  Advanced options" } else { "+  Advanced options" }, Message::Advanced),
                ].spacing(16);
                if self.form.advanced {
                    form = form.push(container(text("Production mapping: authentication, identity file, proxy, tunnels and SSH policy belong here. These are not wired in this preview; do not enter credentials.").size(13).color(MUTED)).padding(12).style(card));
                }
                if let Some(error) = &self.form.error { form = form.push(text(error).color(Color::from_rgb8(255, 153, 164))); }
                form.push(text("DESIGN PREVIEW: creates a sample session only. No SSH connection.").size(12).color(BLUE))
                    .push(row![space::horizontal(), action("Cancel", Message::CloseDialog), action("Open preview", Message::Submit).style(primary)].spacing(10)).into()
            }
            Dialog::Commands => column![
                text("Commands").size(24), text("App actions only. Nothing is sent to a remote host.").color(MUTED),
                action("New connection                      Ctrl / Cmd + N", Message::New).width(Fill),
                action("Open sample workspace", Message::Open(0)).width(Fill),
                action("Split active pane right", Message::Split(pane_grid::Axis::Vertical)).width(Fill),
                action("Toggle files and transfers", Message::ToggleFiles).width(Fill),
                action("Close", Message::CloseDialog),
            ].spacing(14).into(),
            Dialog::Close(index) => column![
                text("Close this session?").size(24),
                text("This closes the sample workspace and all its panes. No remote processes are running.").color(MUTED),
                row![space::horizontal(), action("Cancel", Message::CloseDialog), action("Close session", Message::ConfirmClose(*index)).style(button::danger)].spacing(10),
            ].spacing(20).into(),
            Dialog::About => column![
                text("Design before decoration.").size(24),
                text("A native Iced interaction prototype for Inspirum.\n\nWorking: session search, profile form, tabs, draggable/resizable splits, file selection, preview transfer queue and UI scaling.\n\nNot implemented here: SSH, PTY rendering, real SFTP, credentials, persistent profiles or terminal performance benchmarks.\n\nThe production application is unchanged. All displayed server names and terminal output are fixtures.").color(MUTED),
                action("Back to workspace", Message::CloseDialog).style(primary),
            ].spacing(20).into(),
        };
        container(scrollable(body)).width(580).max_height(700).padding(28).style(active_card).into()
    }
}
fn field<'a>(label: &'a str, hint: &'a str, value: &'a str, message: fn(String) -> Message) -> Element<'a, Message> {
    column![text(label).size(13).color(MUTED), text_input(hint, value).on_input(message).on_submit(Message::Submit).padding(11)].spacing(7).into()
}
fn action<'a>(label: impl Into<String>, message: Message) -> button::Button<'a, Message> {
    button(text(label.into()).size(14)).padding([8, 12]).style(quiet).on_press(message)
}
fn surface(_: &Theme) -> container::Style {
    container::Style { background: Some(PANEL.into()), text_color: Some(FG), ..Default::default() }
}
fn card(_: &Theme) -> container::Style {
    container::Style { background: Some(BG.into()), text_color: Some(FG), border: Border { color: LINE, width: 1.0, radius: 6.0.into() }, ..Default::default() }
}
fn active_card(theme: &Theme) -> container::Style {
    container::Style { border: Border { color: BLUE, width: 1.0, radius: 6.0.into() }, ..card(theme) }
}
fn quiet(_: &Theme, status: button::Status) -> button::Style {
    button::Style { background: if matches!(status, button::Status::Hovered | button::Status::Pressed) { Some(ELEVATED.into()) } else { None }, text_color: if status == button::Status::Disabled { MUTED } else { FG }, border: Border { radius: 5.0.into(), ..Default::default() }, ..Default::default() }
}
fn selected_button(theme: &Theme, status: button::Status) -> button::Style {
    button::Style { background: Some(ELEVATED.into()), text_color: BLUE, ..quiet(theme, status) }
}
fn primary(theme: &Theme, status: button::Status) -> button::Style {
    button::Style { background: Some(BLUE.into()), text_color: BG, ..quiet(theme, status) }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn opening_existing_session_focuses_it() {
        let mut app = App::empty(); app.open(0); app.open(1); app.open(0);
        assert_eq!(app.tabs.len(), 2); assert_eq!(app.active, 0);
    }
    #[test]
    fn splits_are_bounded_and_independent_per_tab() {
        let mut app = App::empty(); app.open(0);
        for _ in 0..10 { app.tabs[0].split(pane_grid::Axis::Vertical); }
        assert_eq!(app.tabs[0].panes.len(), 4); app.open(1);
        assert_eq!(app.tabs[1].panes.len(), 1);
    }
    #[test]
    fn files_are_unavailable_without_a_session() {
        let mut app = App::empty(); app.toggle_files(); assert!(app.files_dock.is_none());
        app.open(0); app.toggle_files(); assert!(app.files_dock.is_some());
        app.toggle_files(); assert!(app.files_dock.is_none());
    }
    #[test]
    fn last_tab_close_clears_dock() {
        let mut app = App::empty(); app.open(0); app.toggle_files();
        let _ = app.update(Message::ConfirmClose(0));
        assert!(app.tabs.is_empty()); assert!(app.files_dock.is_none());
    }
    #[test]
    fn invalid_form_never_creates_a_tab() {
        let mut app = App::empty(); let _ = app.update(Message::Submit);
        assert!(app.tabs.is_empty()); assert!(app.form.error.is_some());
    }
    #[test]
    fn cancel_discards_form_without_saving() {
        let mut app = App::empty(); app.form.host = "example.test".into();
        let _ = app.update(Message::CloseDialog);
        assert!(app.form.host.is_empty()); assert_eq!(app.profiles.len(), 4);
    }
    #[test]
    fn scale_is_bounded() {
        let mut app = App::empty(); let _ = app.update(Message::Scale(100.0));
        assert_eq!(app.scale, 1.5); let _ = app.update(Message::Scale(-100.0)); assert_eq!(app.scale, 0.85);
    }
}
