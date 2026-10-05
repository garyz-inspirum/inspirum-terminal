//! Small native connection/profile interface; terminal mechanics stay upstream.
use crate::{Session, load_sessions, save_sessions, terminal};
use eframe::egui;
use egui_term::{PtyEvent, TerminalBackend, TerminalView};
use std::{
    path::PathBuf,
    sync::mpsc::{self, Receiver, Sender},
};

struct Tab {
    id: u64,
    name: String,
    terminal: TerminalBackend,
    exited: bool,
}
fn terminal_accepts_keyboard(owner: Option<u64>, id: u64, exited: bool) -> bool {
    owner == Some(id) && !exited
}
pub struct App {
    path: PathBuf,
    config: Option<PathBuf>,
    profiles: Vec<Session>,
    draft: Session,
    port: String,
    error: String,
    writable: bool,
    tabs: Vec<Tab>,
    active: Option<u64>,
    terminal_focus: Option<u64>,
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
            draft: Session::default(),
            port: String::new(),
            error,
            writable,
            tabs: Vec::new(),
            active: None,
            terminal_focus: None,
            next_id: 1,
            tx,
            rx,
        }
    }
    pub fn storage_writable(&self) -> bool {
        self.writable
    }
    fn validated_draft(&self) -> anyhow::Result<Session> {
        let mut session = self.draft.clone();
        session.port = if self.port.trim().is_empty() {
            None
        } else {
            Some(self.port.parse()?)
        };
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
        egui::SidePanel::left("connections").resizable(true).default_width(255.0).show(ctx, |ui| {
            ui.heading("Inspirum Terminal");
            ui.label("Early SSH slice • system OpenSSH");
            ui.separator();
            ui.label("Saved sessions");
            egui::ScrollArea::vertical().max_height(150.0).show(ui, |ui| {
                for profile in &self.profiles {
                    if ui.button(&profile.name).clicked() {
                        self.terminal_focus = None;
                        self.draft = profile.clone();
                        self.port = profile.port.map(|p| p.to_string()).unwrap_or_default();
                    }
                }
            });
            ui.separator();
            ui.label("Name");
            if ui.text_edit_singleline(&mut self.draft.name).has_focus() { self.terminal_focus = None; }
            ui.label("Host / SSH config alias");
            if ui.text_edit_singleline(&mut self.draft.host).has_focus() { self.terminal_focus = None; }
            ui.label("Username (blank: SSH config/default)");
            if ui.text_edit_singleline(&mut self.draft.user).has_focus() { self.terminal_focus = None; }
            ui.label("Port (blank: SSH config/default)");
            if ui.text_edit_singleline(&mut self.port).has_focus() { self.terminal_focus = None; }
            if ui.checkbox(&mut self.draft.strict, "Require already trusted host key").clicked() { self.terminal_focus = None; }
            ui.small("Unchecked: OpenSSH asks before trusting a new host. Changed host keys are rejected.");
            ui.horizontal(|ui| {
                if ui.add_enabled(self.writable, egui::Button::new("Save profile")).clicked() {
                    self.terminal_focus = None;
                    let result = self.validated_draft().and_then(|session| {
                        let mut next = self.profiles.clone();
                        if let Some(existing) = next.iter_mut().find(|p| p.name == session.name) { *existing = session; }
                        else { next.push(session); }
                        save_sessions(&self.path, &next)?;
                        self.profiles = next;
                        Ok(())
                    });
                    self.error = result.err().map(|e| format!("{e:#}")).unwrap_or_default();
                }
                if ui.add_enabled(self.tabs.len() < 16, egui::Button::new("Connect")).clicked() {
                    let result = self.validated_draft().and_then(|session| {
                        let terminal = terminal::connect(self.next_id, ctx.clone(), self.tx.clone(), &session, self.config.as_deref())?;
                        self.tabs.push(Tab { id: self.next_id, name: session.name, terminal, exited: false });
                        self.active = Some(self.next_id); self.terminal_focus = Some(self.next_id); self.next_id += 1;
                        Ok(())
                    });
                    self.error = result.err().map(|e| format!("{e:#}")).unwrap_or_default();
                }
            });
            if !self.error.is_empty() { ui.colored_label(egui::Color32::LIGHT_RED, &self.error); }
            ui.separator();
            ui.small("Authentication happens only inside OpenSSH's PTY. No passwords or private keys are saved by this app.");
            ui.small(format!("Profiles: {}", self.path.display()));
            if let Some(config) = &self.config { ui.small(format!("SSH config: {}", config.display())); }
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
                    if ui
                        .small_button("×")
                        .on_hover_text("Disconnect and close terminal")
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
        egui::CentralPanel::default().show(ctx, |ui| {
            let terminal_focus = self.terminal_focus;
            if let Some(tab) = self.tabs.iter_mut().find(|tab| Some(tab.id) == self.active) {
                let view = TerminalView::new(ui, &mut tab.terminal)
                    .set_focus(terminal_accepts_keyboard(terminal_focus, tab.id, tab.exited));
                let response = ui.add(view);
                if response.clicked() && !tab.exited { self.terminal_focus = Some(tab.id); }
            } else {
                ui.heading("Connect to an SSH server");
                ui.label("Enter a host or existing ~/.ssh/config alias, then Connect.");
                ui.label("Click the terminal to type. Close a tab to disconnect.");
                ui.label("Verify host key fingerprints through an independent trusted channel before accepting.");
                ui.label("No sessions are automatically connected on startup.");
            }
        });
    }
}
impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.ui(ctx);
    }
}
