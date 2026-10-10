//! Iced application tools backed by the same policy modules as the legacy UI.
mod ssh_tools;
use super::*;
use crate::{
    appearance::{
        self, AppearanceSettings, TerminalAppearance, TerminalCursorStyle, TerminalFontFamily,
        TerminalPalette,
    },
    history::{HistoryRow, HistoryState},
    startup::{self, StartupBehavior, StartupSettings},
    tab_management::{self, BulkCloseMode, TabVisualLabel},
    workspace::{self, SyncInputState, WorkspaceLayout},
};
use iced::widget::column;
use iced::widget::{checkbox, slider};
use ssh_tools::SshField;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Panel {
    Profiles,
    Workspace,
    History,
    Appearance,
    Ssh,
    Scp,
    Diagnostics,
    Snippets,
}
impl Panel {
    pub(super) const ALL: [Self; 8] = [
        Self::Profiles,
        Self::Workspace,
        Self::History,
        Self::Appearance,
        Self::Ssh,
        Self::Scp,
        Self::Diagnostics,
        Self::Snippets,
    ];
    fn name(self) -> &'static str {
        match self {
            Self::Profiles => "Profiles",
            Self::Workspace => "Workspace",
            Self::History => "History",
            Self::Appearance => "Appearance",
            Self::Ssh => "SSH tools",
            Self::Scp => "SCP",
            Self::Diagnostics => "Diagnostics",
            Self::Snippets => "Snippets",
        }
    }
}
#[derive(Clone, Copy, Debug)]
pub(super) enum Field {
    ProfilePath,
    WorkspacePath,
    TabQuery,
    LogPath,
    Foreground,
    Background,
    SnippetPath,
    Completion,
    KnownHosts,
    TmuxName,
    ScpLocal,
    ScpRemote,
    ReportPath,
}
#[derive(Clone, Debug)]
pub(super) enum Confirm {
    DeleteProfile(String),
    ReplaceProfiles(PathBuf),
    CloseTabs(Vec<u64>),
    Send {
        source: u64,
        targets: Vec<u64>,
        text: String,
    },
    RemoveKey(terminal::HostKeyTarget, Option<PathBuf>),
    CloseMaster(Session),
    Scp {
        session: Session,
        upload: bool,
        local: PathBuf,
        remote: String,
        overwrite: bool,
    },
}
#[derive(Clone)]
pub(super) enum Data {
    Notice(String),
    Profiles(Vec<Session>),
    DraftSaved(Vec<Session>, Box<Session>),
    Layout(WorkspaceLayout),
    Snippets(SnippetLibrary),
    Text(String),
    Report(String),
    Host(String, terminal::HostKeyTarget, String),
    Tmux(String, Vec<crate::tmux::TmuxSession>),
    Tunnel(String, Arc<Mutex<terminal::TunnelProcess>>),
    TunnelStatus(String, bool),
}
impl std::fmt::Debug for Data {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ToolResult")
    }
}
#[derive(Clone, Debug)]
pub(super) enum Action {
    Open(Panel),
    Set(Field, String),
    ProfileSelect(String),
    Duplicate(String),
    Delete(String),
    ExportProfiles,
    ReplaceProfiles,
    SaveDraft,
    Confirm,
    CancelConfirm,
    StartupNone,
    StartupProfile(String),
    StartupWorkspace,
    SaveWorkspace,
    LoadWorkspace,
    RestoreWorkspace,
    WorkspaceReconnect(bool),
    TabSelect(u64),
    TabMove(isize),
    TabLabel(TabVisualLabel),
    TabToggle(u64, bool),
    CloseTabs(BulkCloseMode),
    SyncTarget(u64, bool),
    SyncArm(bool),
    HistoryQuery(String),
    HistoryNext(bool),
    HistorySelect(usize),
    HistoryMark,
    HistoryTimestamps(bool),
    HistoryFold(u64),
    HistoryRemove(u64),
    HistoryMore,
    LogStart,
    LogReady(u64, PathBuf, Result<(), String>),
    LogWritten(u64, PathBuf, String, Result<(), String>),
    LogStop,
    FontSize(f32),
    FontFamily(TerminalFontFamily),
    Palette(TerminalPalette),
    Cursor(TerminalCursorStyle),
    Opacity(f32),
    Pointer(u8, bool),
    AppearanceSave(bool),
    AppearanceReset(bool),
    PastePolicy(PastePolicy),
    ImportSnippets,
    ExportSnippets,
    CompleteSnippet,
    SenderSync(bool),
    SendCommand,
    JobDone(u64, Result<Data, String>),
    SshText(SshField, String),
    MasterMode(crate::ControlMasterMode),
    SshPolicy(u8, Option<bool>),
    SaveSshPolicy,
    TunnelExposure(bool),
    InspectKey,
    AskRemoveKey,
    MasterCheck,
    MasterClose,
    StartTunnels,
    StopTunnels,
    CheckTunnels,
    TmuxRefresh,
    TmuxAttach(String),
    TmuxCreate,
    SftpTerminal,
    ScpStart(bool),
    ScpCancel,
    ScpOverwrite(bool),
    ScpProgress(u64, u64, u64),
    ScpDone(u64, Result<String, String>),
    GenerateReport,
    ExportReport,
    ClearErrors,
}

pub(super) struct State {
    pub panel: Panel,
    pub target: Option<Session>,
    pub profile_path: String,
    pub profile_selected: Option<String>,
    pub workspace_path: String,
    pub workspace: Option<WorkspaceLayout>,
    pub workspace_reconnect: bool,
    pub startup: StartupSettings,
    pub startup_pending: bool,
    pub tab_query: String,
    pub selected_tabs: BTreeSet<u64>,
    pub tab_labels: BTreeMap<u64, TabVisualLabel>,
    pub sync: SyncInputState,
    pub histories: BTreeMap<u64, HistoryState>,
    pub history_owner: Option<u64>,
    pub history_visible: usize,
    pub timestamps: bool,
    pub log_path: String,
    pub logs: BTreeMap<u64, (PathBuf, String)>,
    pub log_pending: BTreeSet<u64>,
    pub appearance: AppearanceSettings,
    pub appearance_draft: TerminalAppearance,
    pub snippet_path: String,
    pub completion: String,
    pub sender_sync: bool,
    pub confirm: Option<Confirm>,
    pub busy: bool,
    pub generation: u64,
    pub output: String,
    pub known_hosts: String,
    pub host_target: Option<terminal::HostKeyTarget>,
    pub tmux: Vec<crate::tmux::TmuxSession>,
    pub tmux_name: String,
    pub control_persist: String,
    pub tunnel_exposure: bool,
    pub tunnel: Option<Arc<Mutex<terminal::TunnelProcess>>>,
    pub tunnel_target: Option<String>,
    pub scp_local: String,
    pub scp_remote: String,
    pub scp_overwrite: bool,
    pub scp_cancel: Option<Arc<AtomicBool>>,
    pub scp_id: u64,
    pub scp_progress: (u64, u64),
    pub scp_status: String,
    pub report: String,
    pub report_path: String,
    pub errors: crate::support::SanitizedErrorHistory,
    pub load_errors: Vec<String>,
}
impl State {
    pub fn load(path: &std::path::Path) -> Self {
        let mut load_errors = Vec::new();
        let startup = startup::load_settings(&startup::settings_path(path)).unwrap_or_else(|e| {
            load_errors.push(format!("Could not load startup preferences: {e:#}"));
            StartupSettings::default()
        });
        let appearance = appearance::load_settings(&appearance::settings_path(path))
            .unwrap_or_else(|e| {
                load_errors.push(format!("Could not load appearance preferences: {e:#}"));
                AppearanceSettings::default()
            });
        let workspace = match &startup.behavior {
            StartupBehavior::Workspace { path } => {
                workspace::load_layout(std::path::Path::new(path))
                    .map_err(|e| {
                        load_errors.push(format!("Could not load startup workspace: {e:#}"))
                    })
                    .ok()
            }
            _ => None,
        };
        let workspace_path = match &startup.behavior {
            StartupBehavior::Workspace { path } => path.clone(),
            _ => String::new(),
        };
        let workspace_reconnect = workspace.as_ref().is_some_and(|w| w.reconnect_on_restore);
        Self {
            panel: Panel::Profiles,
            target: None,
            profile_path: String::new(),
            profile_selected: None,
            workspace_path,
            workspace,
            workspace_reconnect,
            startup,
            startup_pending: true,
            tab_query: String::new(),
            selected_tabs: BTreeSet::new(),
            tab_labels: BTreeMap::new(),
            sync: SyncInputState::default(),
            histories: BTreeMap::new(),
            history_owner: None,
            history_visible: 200,
            timestamps: false,
            log_path: String::new(),
            logs: BTreeMap::new(),
            log_pending: BTreeSet::new(),
            appearance_draft: appearance.global.clone(),
            appearance,
            snippet_path: String::new(),
            completion: String::new(),
            sender_sync: false,
            confirm: None,
            busy: false,
            generation: 0,
            output: String::new(),
            known_hosts: String::new(),
            host_target: None,
            tmux: Vec::new(),
            tmux_name: String::new(),
            control_persist: String::new(),
            tunnel_exposure: false,
            tunnel: None,
            tunnel_target: None,
            scp_local: String::new(),
            scp_remote: String::new(),
            scp_overwrite: false,
            scp_cancel: None,
            scp_id: 0,
            scp_progress: (0, 0),
            scp_status: String::new(),
            report: String::new(),
            report_path: String::new(),
            errors: Default::default(),
            load_errors,
        }
    }
}
impl Drop for State {
    fn drop(&mut self) {
        if let Some(cancel) = &self.scp_cancel {
            cancel.store(true, Ordering::Release);
        }
    }
}

fn msg(action: Action) -> Message {
    Message::Tool(action)
}
fn act(label: impl Into<String>, action: Action) -> iced::widget::Button<'static, Message> {
    super::action(label, msg(action))
}
fn input<'a>(label: &'a str, value: &'a str, field: Field) -> Element<'a, Message> {
    column![
        text(label).size(12).color(MUTED),
        text_input(label, value)
            .padding(8)
            .on_input(move |v| msg(Action::Set(field, v)))
    ]
    .spacing(5)
    .into()
}
impl App {
    pub(super) fn tool_job<F>(&mut self, job: F) -> Task<Message>
    where
        F: FnOnce() -> Result<Data, String> + Send + 'static,
    {
        if self.tools.busy {
            return Task::none();
        }
        self.tools.busy = true;
        self.tools.generation = self.tools.generation.wrapping_add(1);
        let generation = self.tools.generation;
        Task::perform(run_blocking_result(job), move |r| {
            msg(Action::JobDone(generation, r))
        })
    }
    pub(super) fn focused_profile(&self) -> Option<Session> {
        self.tabs
            .get(self.active)
            .and_then(|t| t.panes.get(t.focus))
            .map(|p| p.profile.clone())
    }
    pub(super) fn open_profile_value(&mut self, profile: Session) {
        self.tools.sync.set_armed(false);
        let pane = self.new_terminal_pane(profile.clone());
        self.tabs.push(Workspace::new(profile, pane));
        self.active = self.tabs.len() - 1;
        self.dialog = None;
    }
    pub(super) fn start_configured_profile(&mut self) {
        if !self.tools.startup_pending {
            return;
        }
        self.tools.startup_pending = false;
        if let StartupBehavior::Profile { profile } = &self.tools.startup.behavior {
            if let Some(session) = self
                .profiles
                .iter()
                .find(|s| session_profile_key(s) == *profile)
                .cloned()
            {
                self.open_profile_value(session);
            } else {
                self.status = "Configured startup profile no longer exists.".into();
            }
        }
    }
    fn workspace_id(tab: &Workspace) -> u64 {
        tab.id
    }
    fn history_refresh(&mut self) {
        if let Some(tab) = self.tabs.get_mut(self.active)
            && let Some(pane) = tab.panes.get_mut(tab.focus)
            && let Some(terminal) = &pane.terminal
        {
            self.tools.history_owner = Some(pane.id);
            self.tools
                .histories
                .entry(pane.id)
                .or_default()
                .update_snapshot(terminal.history_lines());
        }
    }
    fn history_jump(&mut self) -> Task<Message> {
        if let Some(id) = self.tools.history_owner
            && self.focused_pane_matches(id)
            && let Some(hit) = self
                .tools
                .histories
                .get(&id)
                .and_then(|s| s.selected_hit())
                .cloned()
            && let Some(tab) = self.tabs.get_mut(self.active)
            && let Some(pane) = tab.panes.get_mut(tab.focus)
            && let Some(terminal) = &mut pane.terminal
        {
            terminal.scroll_to_history_index(hit.line_number.saturating_sub(1));
            return self.defer_terminal_refresh(id);
        }
        Task::none()
    }
    pub(super) fn tool_update(&mut self, action: Action) -> Task<Message> {
        let writes_profiles = matches!(&action, Action::SaveDraft)
            || (matches!(&action, Action::Confirm)
                && matches!(
                    &self.tools.confirm,
                    Some(Confirm::DeleteProfile(_) | Confirm::ReplaceProfiles(_))
                ));
        if writes_profiles && !self.profiles_writable {
            self.tools.output =
                "Profile storage could not be loaded; repair it before saving or importing.".into();
            return Task::none();
        }
        match action {
            Action::Open(panel) => {
                self.tools.panel = panel;
                self.tools.confirm = None;
                let target = self.tools.target.clone().or_else(|| self.focused_profile());
                if self.tools.target.as_ref().map(session_profile_key)
                    != target.as_ref().map(session_profile_key)
                {
                    self.tools.host_target = None;
                    self.tools.tmux.clear();
                    self.tools.output.clear();
                }
                self.tools.control_persist = target
                    .as_ref()
                    .and_then(|s| s.ssh.control_persist_seconds)
                    .map(|v| v.to_string())
                    .unwrap_or_default();
                self.tools.target = target.or_else(|| {
                    self.tools.profile_selected.as_ref().and_then(|key| {
                        self.profiles
                            .iter()
                            .find(|s| session_profile_key(s) == *key)
                            .cloned()
                    })
                });
                if panel == Panel::Appearance {
                    self.tools.appearance_draft = self
                        .tools
                        .target
                        .as_ref()
                        .map(|s| self.tools.appearance.effective_for(&session_profile_key(s)))
                        .unwrap_or_else(|| self.tools.appearance.global.clone());
                }
                if panel == Panel::History {
                    self.history_refresh();
                }
                self.dialog = Some(Dialog::Tools);
            }
            Action::Set(field, value) => match field {
                Field::ProfilePath => self.tools.profile_path = value,
                Field::WorkspacePath => self.tools.workspace_path = value,
                Field::TabQuery => self.tools.tab_query = value,
                Field::LogPath => self.tools.log_path = value,
                Field::Foreground => {
                    self.tools.appearance_draft.foreground = (!value.is_empty()).then_some(value)
                }
                Field::Background => {
                    self.tools.appearance_draft.background = (!value.is_empty()).then_some(value)
                }
                Field::SnippetPath => self.tools.snippet_path = value,
                Field::Completion => self.tools.completion = value,
                Field::KnownHosts => {
                    self.tools.known_hosts = value;
                    self.tools.host_target = None;
                }
                Field::TmuxName => self.tools.tmux_name = value,
                Field::ScpLocal => self.tools.scp_local = value,
                Field::ScpRemote => self.tools.scp_remote = value,
                Field::ReportPath => self.tools.report_path = value,
            },
            Action::ProfileSelect(key) => {
                self.tools.host_target = None;
                self.tools.tmux.clear();
                self.tools.output.clear();
                self.tools.tunnel_exposure = false;
                self.tools.target = self
                    .profiles
                    .iter()
                    .find(|s| session_profile_key(s) == key)
                    .cloned();
                self.tools.control_persist = self
                    .tools
                    .target
                    .as_ref()
                    .and_then(|s| s.ssh.control_persist_seconds)
                    .map(|v| v.to_string())
                    .unwrap_or_default();
                self.tools.profile_selected = Some(key);
            }
            Action::Duplicate(key) => {
                if let Some(profile) = self.profiles.iter().find(|s| session_profile_key(s) == key)
                {
                    self.form = ConnectionForm::from_session(&crate::duplicate_session_draft(
                        &self.profiles,
                        profile,
                    ));
                    self.editing_profile = None;
                    self.dialog = Some(Dialog::Connection);
                }
            }
            Action::Delete(key) => self.tools.confirm = Some(Confirm::DeleteProfile(key)),
            Action::ExportProfiles => {
                let path = PathBuf::from(self.tools.profile_path.trim());
                let profiles = self.profiles.clone();
                return self.tool_job(move || {
                    crate::export_sessions(&path, &profiles)
                        .map(|_| Data::Notice("Profiles exported without overwriting.".into()))
                        .map_err(|e| format!("{e:#}"))
                });
            }
            Action::ReplaceProfiles => {
                self.tools.confirm = Some(Confirm::ReplaceProfiles(PathBuf::from(
                    self.tools.profile_path.trim(),
                )))
            }
            Action::SaveDraft => {
                match self.form.session().and_then(|s| {
                    save_session_edit(&self.profiles, self.editing_profile.as_deref(), s.clone())
                        .map(|profiles| (profiles, s))
                }) {
                    Ok((profiles, session)) => {
                        let path = self.profiles_path.clone();
                        return self.tool_job(move || {
                            save_sessions(&path, &profiles)
                                .map(|_| Data::DraftSaved(profiles, Box::new(session)))
                                .map_err(|e| format!("{e:#}"))
                        });
                    }
                    Err(e) => self.form.error = Some(format!("{e:#}")),
                }
            }
            Action::CancelConfirm => self.tools.confirm = None,
            Action::Confirm => {
                if self.tools.busy {
                    return Task::none();
                }
                if let Some(confirm) = self.tools.confirm.take() {
                    match confirm {
                        Confirm::DeleteProfile(key) => {
                            match crate::delete_session(&self.profiles, &key) {
                                Ok(profiles) => {
                                    let path = self.profiles_path.clone();
                                    return self.tool_job(move || {
                                        save_sessions(&path, &profiles)
                                            .map(|_| Data::Profiles(profiles))
                                            .map_err(|e| format!("{e:#}"))
                                    });
                                }
                                Err(e) => self.status = format!("{e:#}"),
                            }
                        }
                        Confirm::ReplaceProfiles(source) => {
                            let path = self.profiles_path.clone();
                            return self.tool_job(move || {
                                let profiles =
                                    import_sessions(&source, &[], SessionImportMode::Replace)
                                        .map_err(|e| format!("{e:#}"))?;
                                save_sessions(&path, &profiles).map_err(|e| format!("{e:#}"))?;
                                Ok(Data::Profiles(profiles))
                            });
                        }
                        Confirm::CloseTabs(ids) => {
                            let closing: BTreeSet<_> = ids.into_iter().collect();
                            let dirty = self.remote_editor.as_ref().is_some_and(|e| {
                                (e.dirty || e.saving)
                                    && self.tabs.iter().any(|t| {
                                        closing.contains(&Self::workspace_id(t))
                                            && session_profile_key(&t.profile) == e.session_key
                                    })
                            });
                            if dirty {
                                self.status="Save or discard remote editor changes before closing these tabs.".into();
                                return Task::none();
                            }
                            let active = self.tabs.get(self.active).map(Self::workspace_id);
                            self.tabs
                                .retain(|t| !closing.contains(&Self::workspace_id(t)));
                            self.active = active
                                .and_then(|id| {
                                    self.tabs.iter().position(|t| Self::workspace_id(t) == id)
                                })
                                .unwrap_or(0)
                                .min(self.tabs.len().saturating_sub(1));
                            self.prune_tool_panes();
                            self.dialog = None;
                            if self.tabs.is_empty() && self.files_dock.is_some() {
                                self.toggle_files();
                            }
                            if self.files_dock.is_some() {
                                return self.reload_files();
                            }
                        }
                        Confirm::Send {
                            source,
                            targets,
                            text,
                        } => {
                            if self.focused_pane_matches(source)
                                && targets.iter().all(|id| self.active_terminal_matches(*id))
                            {
                                for id in targets {
                                    self.command_terminal(
                                        id,
                                        terminal_core::BackendCommand::Write(
                                            text.as_bytes().to_vec(),
                                        ),
                                    );
                                }
                                self.status =
                                    "Command sent to the explicitly selected panes.".into();
                            } else {
                                self.status =
                                    "Send cancelled because terminal targets changed.".into();
                            }
                        }
                        other => return self.ssh_confirm(other),
                    }
                }
            }
            Action::StartupNone | Action::StartupProfile(_) | Action::StartupWorkspace => {
                let behavior = match action {
                    Action::StartupProfile(profile) => StartupBehavior::Profile { profile },
                    Action::StartupWorkspace => StartupBehavior::Workspace {
                        path: self.tools.workspace_path.clone(),
                    },
                    _ => StartupBehavior::None,
                };
                let settings = StartupSettings { behavior };
                let path = startup::settings_path(&self.profiles_path);
                match startup::save_settings(&path, &settings) {
                    Ok(()) => {
                        self.tools.startup = settings;
                        self.status = "Startup preference saved for the next launch.".into();
                    }
                    Err(e) => self.status = format!("{e:#}"),
                }
            }
            Action::WorkspaceReconnect(v) => self.tools.workspace_reconnect = v,
            Action::SaveWorkspace => {
                if let Some(tab) = self.tabs.get(self.active) {
                    if tab.panes.iter().any(|(_, pane)| pane.sftp) {
                        self.status = "Saved SSH workspaces cannot contain SFTP terminals.".into();
                        return Task::none();
                    }
                    let mut profiles = Vec::new();
                    let tree =
                        capture_workspace_tree(tab.panes.layout(), &tab.panes, &mut profiles);
                    let layout = WorkspaceLayout {
                        version: 1,
                        axis: crate::workspace::SplitAxis::Vertical,
                        reconnect_on_restore: self.tools.workspace_reconnect,
                        panes: profiles,
                        tree: Some(tree),
                    };
                    let path = PathBuf::from(self.tools.workspace_path.trim());
                    return self.tool_job(move || {
                        workspace::save_layout(&path, &layout)
                            .map(|_| Data::Notice("Workspace saved.".into()))
                            .map_err(|e| format!("{e:#}"))
                    });
                }
            }
            Action::LoadWorkspace => {
                let path = PathBuf::from(self.tools.workspace_path.trim());
                return self.tool_job(move || {
                    workspace::load_layout(&path)
                        .map(Data::Layout)
                        .map_err(|e| format!("{e:#}"))
                });
            }
            Action::RestoreWorkspace => {
                if let Some(layout) = self.tools.workspace.clone() {
                    if !layout.may_reconnect(true) {
                        self.status="Loaded workspace does not permit reconnect. Save a layout with reconnect enabled first.".into();
                        return Task::none();
                    }
                    if let Err(e) = layout.validate() {
                        self.status = format!("{e:#}");
                        return Task::none();
                    }
                    let profile = layout.panes[0].clone();
                    let panes: Vec<_> = layout
                        .panes
                        .into_iter()
                        .map(|p| Some(self.new_terminal_pane(p)))
                        .collect();
                    let id = panes[0].as_ref().expect("validated workspace").id;
                    let tree = layout
                        .tree
                        .unwrap_or_else(|| fallback_workspace_tree(panes.len(), layout.axis));
                    let mut panes = panes;
                    let config = restore_workspace_tree(&tree, &mut panes);
                    let panes = pane_grid::State::with_configuration(config);
                    let focus = *panes.iter().next().expect("validated workspace").0;
                    self.tools.sync.set_armed(false);
                    self.tabs.push(Workspace {
                        id,
                        profile,
                        panes,
                        focus,
                    });
                    self.active = self.tabs.len() - 1;
                    self.dialog = None;
                }
            }

            Action::TabSelect(id) => {
                if let Some(index) = self.tabs.iter().position(|t| Self::workspace_id(t) == id) {
                    self.activate_tab(index);
                    self.dialog = None;
                    if self.files_dock.is_some() {
                        return self.reload_files();
                    }
                }
            }
            Action::TabMove(delta) => {
                let target = if delta < 0 {
                    self.active.saturating_sub(1)
                } else {
                    (self.active + 1).min(self.tabs.len().saturating_sub(1))
                };
                if self.active < self.tabs.len() {
                    self.tabs.swap(self.active, target);
                    self.active = target;
                }
            }
            Action::TabLabel(label) => {
                if let Some(tab) = self.tabs.get(self.active) {
                    self.tools.tab_labels.insert(Self::workspace_id(tab), label);
                }
            }
            Action::TabToggle(id, on) => {
                if on {
                    self.tools.selected_tabs.insert(id);
                } else {
                    self.tools.selected_tabs.remove(&id);
                }
            }
            Action::CloseTabs(mode) => {
                let order: Vec<_> = self.tabs.iter().map(Self::workspace_id).collect();
                self.tools.confirm = Some(Confirm::CloseTabs(tab_management::bulk_close_ids(
                    &order,
                    &self.tools.selected_tabs,
                    mode,
                )));
            }
            Action::SyncTarget(id, v) => {
                if self.active_terminal_matches(id) {
                    self.tools.sync.set_target(id, v);
                }
            }
            Action::SyncArm(v) => {
                self.tools.sync.set_armed(v);
                self.status = if self.tools.sync.armed() {
                    "Synchronized input ARMED for explicitly selected panes."
                } else {
                    "Synchronized input disarmed."
                }
                .into();
            }
            Action::HistoryQuery(q) => {
                if let Some(id) = self.tools.history_owner {
                    self.tools.histories.entry(id).or_default().set_query(q);
                }
                return self.history_jump();
            }
            Action::HistoryNext(forward) => {
                if let Some(s) = self
                    .tools
                    .history_owner
                    .and_then(|id| self.tools.histories.get_mut(&id))
                {
                    if forward {
                        s.select_next_hit();
                    } else {
                        s.select_previous_hit();
                    }
                }
                return self.history_jump();
            }
            Action::HistorySelect(index) => {
                if let Some(s) = self
                    .tools
                    .history_owner
                    .and_then(|id| self.tools.histories.get_mut(&id))
                {
                    s.select_hit(index);
                }
                return self.history_jump();
            }
            Action::HistoryMark => {
                self.history_refresh();
                if let Some(s) = self
                    .tools
                    .history_owner
                    .and_then(|id| self.tools.histories.get_mut(&id))
                {
                    let stamp = self.tools.timestamps.then(|| {
                        std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_secs()
                    });
                    s.mark_newest_nonempty(stamp);
                }
            }
            Action::HistoryTimestamps(v) => self.tools.timestamps = v,
            Action::HistoryFold(id) => {
                if let Some(s) = self
                    .tools
                    .history_owner
                    .and_then(|id| self.tools.histories.get_mut(&id))
                {
                    s.toggle_fold(id);
                }
            }
            Action::HistoryRemove(id) => {
                if let Some(s) = self
                    .tools
                    .history_owner
                    .and_then(|id| self.tools.histories.get_mut(&id))
                {
                    s.remove_mark(id);
                }
            }
            Action::HistoryMore => {
                self.tools.history_visible = self.tools.history_visible.saturating_add(200)
            }
            Action::LogStart => {
                if let Some(id) = self.focused_terminal_id() {
                    if self.tools.logs.contains_key(&id) || !self.tools.log_pending.insert(id) {
                        return Task::none();
                    }
                    let path = PathBuf::from(self.tools.log_path.trim());
                    let worker_path = path.clone();
                    return Task::perform(
                        run_blocking_result(move || {
                            terminal_ux::start_session_log(&worker_path)
                                .map_err(|e| format!("{e:#}"))
                        }),
                        move |result| msg(Action::LogReady(id, path.clone(), result)),
                    );
                }
            }
            Action::LogReady(id, path, result) => {
                self.tools.log_pending.remove(&id);
                match result {
                    Ok(())
                        if self
                            .tabs
                            .iter()
                            .any(|t| t.panes.iter().any(|(_, p)| p.id == id && !p.exited)) =>
                    {
                        self.tools.logs.insert(id, (path, String::new()));
                        self.status =
                            "Logging enabled for this pane. Remote output can contain secrets."
                                .into();
                        return self.capture_session_logs();
                    }
                    Ok(()) => self.status = "Log created; the original terminal has closed.".into(),
                    Err(e) => {
                        self.tools.errors.record(&e);
                        self.status = e;
                    }
                }
            }
            Action::LogWritten(id, path, screen, result) => {
                self.tools.log_pending.remove(&id);
                if self
                    .tools
                    .logs
                    .get(&id)
                    .is_some_and(|(current, _)| *current == path)
                {
                    match result {
                        Ok(()) => {
                            self.tools.logs.get_mut(&id).expect("matching log").1 = screen;
                            return self.capture_session_logs();
                        }
                        Err(e) => {
                            self.tools.logs.remove(&id);
                            self.tools.errors.record(&e);
                            self.status = format!("Logging stopped: {e}");
                        }
                    }
                }
            }
            Action::LogStop => {
                if let Some(id) = self.focused_terminal_id() {
                    self.tools.logs.remove(&id);
                }
            }
            Action::FontSize(v) => self.tools.appearance_draft.font_size = v,
            Action::FontFamily(v) => self.tools.appearance_draft.font_family = v,
            Action::Palette(v) => self.tools.appearance_draft.palette = v,
            Action::Cursor(v) => self.tools.appearance_draft.cursor_style = v,
            Action::Opacity(v) => self.tools.appearance_draft.opacity = v,
            Action::Pointer(i, v) => {
                let a = &mut self.tools.appearance_draft;
                match i {
                    0 => a.select_to_copy = v,
                    1 => a.middle_click_paste = v,
                    2 => a.right_click_paste = v,
                    _ => a.hide_pointer_while_typing = v,
                }
            }
            Action::AppearanceSave(profile) => {
                let mut settings = self.tools.appearance.clone();
                if profile {
                    if let Some(s) = &self.tools.target {
                        settings.profiles.insert(
                            session_profile_key(s),
                            crate::appearance::full_profile_override(&self.tools.appearance_draft),
                        );
                    }
                } else {
                    settings.global = self.tools.appearance_draft.clone();
                }
                match appearance::save_settings(
                    &appearance::settings_path(&self.profiles_path),
                    &settings,
                ) {
                    Ok(()) => {
                        self.tools.appearance = settings;
                        self.invalidate_terminal_displays();
                    }
                    Err(e) => self.status = format!("{e:#}"),
                }
            }
            Action::AppearanceReset(profile) => {
                let mut settings = self.tools.appearance.clone();
                if profile {
                    if let Some(s) = &self.tools.target {
                        settings.profiles.remove(&session_profile_key(s));
                    }
                } else {
                    settings.global = TerminalAppearance::default();
                }
                self.tools.appearance_draft = settings.global.clone();
                match appearance::save_settings(
                    &appearance::settings_path(&self.profiles_path),
                    &settings,
                ) {
                    Ok(()) => {
                        self.tools.appearance = settings;
                        self.invalidate_terminal_displays();
                    }
                    Err(e) => self.status = format!("{e:#}"),
                }
            }
            Action::PastePolicy(v) => self.paste_policy = v,
            Action::SenderSync(v) => self.tools.sender_sync = v,
            Action::SendCommand => {
                let Some(source) = self.focused_terminal_id() else {
                    return Task::none();
                };
                let targets = if self.tools.sender_sync {
                    self.tabs
                        .get(self.active)
                        .map(|t| {
                            t.panes
                                .iter()
                                .filter(|(_, p)| self.tools.sync.is_target(p.id) && !p.exited)
                                .map(|(_, p)| p.id)
                                .collect()
                        })
                        .unwrap_or_default()
                } else {
                    vec![source]
                };
                if targets.is_empty() {
                    self.status = "Select command target panes in Workspace first.".into();
                    return Task::none();
                }
                let text = self.command_sender.clone();
                if text.is_empty() {
                    self.status = "Command sender text is empty.".into();
                    return Task::none();
                }
                match terminal_ux::classify_paste(self.paste_policy, &text) {
                    PasteDecision::Block => self.status = "Command blocked by paste policy.".into(),
                    _ => {
                        self.tools.confirm = Some(Confirm::Send {
                            source,
                            targets,
                            text,
                        });
                        self.tools.panel = Panel::Snippets;
                        self.dialog = Some(Dialog::Tools);
                    }
                }
            }
            Action::CompleteSnippet => {
                if let Some(text) =
                    command_palette::complete_snippet(&self.tools.completion, &self.snippets)
                {
                    self.stage_command(text);
                } else {
                    self.status = "Completion requires one uniquely matching snippet name.".into();
                }
            }
            Action::ImportSnippets => {
                let path = PathBuf::from(self.tools.snippet_path.trim());
                let destination = self.snippets_path.clone();
                return self.tool_job(move || {
                    let library =
                        command_palette::import_library(&path).map_err(|e| format!("{e:#}"))?;
                    command_palette::save_library(&destination, &library)
                        .map_err(|e| format!("{e:#}"))?;
                    Ok(Data::Snippets(library))
                });
            }
            Action::ExportSnippets => {
                let path = PathBuf::from(self.tools.snippet_path.trim());
                let library = self.snippets.clone();
                return self.tool_job(move || {
                    command_palette::export_library(&path, &library)
                        .map(|_| Data::Notice("Snippets exported without overwriting.".into()))
                        .map_err(|e| format!("{e:#}"))
                });
            }
            Action::JobDone(generation, result) => {
                if generation != self.tools.generation {
                    return Task::none();
                }
                self.tools.busy = false;
                match result {
                    Err(e) => {
                        self.tools.errors.record(&e);
                        self.status = e;
                    }
                    Ok(data) => match data {
                        Data::Notice(s) => self.status = s,
                        Data::Profiles(profiles) => {
                            self.profiles = profiles;
                            self.refresh_profile_matches();
                            self.status = "Profile library saved.".into();
                        }
                        Data::DraftSaved(profiles, session) => {
                            self.profiles = profiles;
                            self.refresh_profile_matches();
                            self.status = "Profile saved without opening a connection.".into();
                            if matches!(self.dialog, Some(Dialog::Connection))
                                && self.form.session().is_ok_and(|draft| draft == *session)
                            {
                                self.dialog = None;
                                self.editing_profile = None;
                            }
                        }
                        Data::Layout(layout) => {
                            self.tools.workspace_reconnect = layout.reconnect_on_restore;
                            self.tools.workspace = Some(layout);
                            self.status =
                                "Loaded metadata only; reconnect requires an explicit action."
                                    .into();
                        }
                        Data::Snippets(library) => {
                            self.snippets = library;
                            self.status = "Snippet library imported.".into();
                        }
                        other => self.ssh_result(other),
                    },
                }
            }
            other => return self.ssh_update(other),
        }
        Task::none()
    }
    pub(super) fn prune_tool_panes(&mut self) {
        let ids: BTreeSet<_> = self
            .tabs
            .iter()
            .flat_map(|t| t.panes.iter().map(|(_, p)| p.id))
            .collect();
        let tabs: BTreeSet<_> = self.tabs.iter().map(|t| t.id).collect();
        self.tools.tab_labels.retain(|id, _| tabs.contains(id));
        self.tools.selected_tabs.retain(|id| tabs.contains(id));
        self.tools.histories.retain(|id, _| ids.contains(id));
        self.tools.logs.retain(|id, _| ids.contains(id));
        self.tools.log_pending.retain(|id| ids.contains(id));
        self.tools.sync.set_armed(false);
        self.tools.sync.retain_panes(&ids);
    }
    pub(super) fn capture_session_logs(&mut self) -> Task<Message> {
        let mut tasks = Vec::new();
        for tab in &self.tabs {
            for (_, pane) in tab.panes.iter() {
                let Some((path, previous)) = self.tools.logs.get(&pane.id) else {
                    continue;
                };
                if self.tools.log_pending.contains(&pane.id) {
                    continue;
                }
                let Some(snapshot) = &pane.display else {
                    continue;
                };
                let screen = snapshot_text(snapshot);
                if screen == *previous {
                    continue;
                }
                let path = path.clone();
                let mut previous = previous.clone();
                let worker_path = path.clone();
                let worker_screen = screen.clone();
                let id = pane.id;
                self.tools.log_pending.insert(id);
                tasks.push(Task::perform(
                    run_blocking_result(move || {
                        terminal_ux::append_screen_snapshot(
                            &worker_path,
                            &mut previous,
                            &worker_screen,
                        )
                        .map(|_| ())
                        .map_err(|e| format!("{e:#}"))
                    }),
                    move |result| msg(Action::LogWritten(id, path.clone(), screen.clone(), result)),
                ));
            }
        }
        Task::batch(tasks)
    }
    pub(super) fn refresh_visible_history(&mut self) {
        if matches!(self.dialog, Some(Dialog::Tools)) && self.tools.panel == Panel::History {
            self.history_refresh();
        }
    }
    pub(super) fn invalidate_terminal_displays(&mut self) {
        for tab in &mut self.tabs {
            for (_, p) in tab.panes.iter_mut() {
                p.display_dirty = true;
                p.display_generation = p.display_generation.wrapping_add(1);
            }
        }
        let sizes: Vec<_> = self
            .tabs
            .iter()
            .flat_map(|t| {
                t.panes
                    .iter()
                    .filter_map(|(_, p)| p.canvas_size.map(|size| (p.id, size)))
            })
            .collect();
        for (id, size) in sizes {
            self.resize_terminal(id, size);
        }
        self.refresh_workspace_displays(self.active);
    }
    pub(super) fn tools_view(&self) -> Element<'_, Message> {
        let mut nav = column![].spacing(5);
        for panels in Panel::ALL.chunks(4) {
            let mut line = row![].spacing(5);
            for panel in panels {
                line = line.push(act(panel.name(), Action::Open(*panel)).style(
                    if *panel == self.tools.panel {
                        selected_button
                    } else {
                        quiet
                    },
                ));
            }
            nav = nav.push(line);
        }
        let content: Element<'_, Message> = if let Some(confirm) = &self.tools.confirm {
            let description = match confirm {
                Confirm::DeleteProfile(key) => {
                    format!("Delete saved profile {key}? Live sessions remain connected.")
                }
                Confirm::ReplaceProfiles(path) => format!(
                    "Replace the entire saved profile library from {}?",
                    path.display()
                ),
                Confirm::CloseTabs(ids) => format!(
                    "Disconnect and close {} selected workspace tabs?",
                    ids.len()
                ),
                Confirm::Send { targets, text, .. } => format!(
                    "Send the following remote text to {} selected pane(s)?\n\n{text}",
                    targets.len()
                ),
                Confirm::RemoveKey(target, path) => format!(
                    "Remove trusted key {} from {}? This does not trust a replacement key.",
                    target.lookup,
                    path.as_ref()
                        .map(|p| p.display().to_string())
                        .unwrap_or_else(|| "the default known_hosts file".into())
                ),
                Confirm::CloseMaster(s) => format!(
                    "Close SSH ControlMaster for {}? Dependent sessions may disconnect.",
                    s.name
                ),
                Confirm::Scp {
                    session,
                    upload,
                    local,
                    remote,
                    overwrite,
                } => format!(
                    "{} with SCP to/from {}. Explicit overwrite is {}.\nLocal: {}\nRemote: {}",
                    if *upload { "Upload" } else { "Download" },
                    session.name,
                    overwrite,
                    local.display(),
                    remote
                ),
            };
            column![
                text(description),
                row![
                    act("Confirm", Action::Confirm).style(primary),
                    act("Cancel", Action::CancelConfirm)
                ]
                .spacing(8)
            ]
            .spacing(16)
            .into()
        } else {
            match self.tools.panel {
                Panel::Profiles => self.profiles_tool_view(),
                Panel::Workspace => self.workspace_tool_view(),
                Panel::History => self.history_tool_view(),
                Panel::Appearance => self.appearance_tool_view(),
                Panel::Snippets => self.snippets_tool_view(),
                _ => self.ssh_tool_view(),
            }
        };
        column![
            nav,
            if self.tools.busy {
                text("Working in background…").color(BLUE)
            } else {
                text("")
            },
            content,
            text(&self.status).size(12).color(MUTED),
            super::action("Close", Message::CloseDialog)
        ]
        .spacing(12)
        .into()
    }
    fn profiles_tool_view(&self) -> Element<'_, Message> {
        let mut list = column![].spacing(5);
        for profile in self
            .profiles
            .iter()
            .filter(|s| session_matches_query(s, &self.query))
            .take(self.profile_visible)
        {
            let key = session_profile_key(profile);
            list = list.push(
                row![
                    act(profile.name.clone(), Action::ProfileSelect(key.clone())),
                    act("Duplicate", Action::Duplicate(key.clone())),
                    act("Delete…", Action::Delete(key.clone())),
                    act("Open on startup", Action::StartupProfile(key))
                ]
                .spacing(6),
            );
        }
        column![
            text("Profile library and startup").size(24),
            text_input("Filter profiles", &self.query)
                .on_input(Message::Search)
                .padding(8),
            scrollable(list).height(240),
            act("Show more profiles", Action::Open(Panel::Profiles))
                .on_press(Message::ShowMoreProfiles),
            input(
                "Import/export JSON path",
                &self.tools.profile_path,
                Field::ProfilePath
            ),
            row![
                act("Export all", Action::ExportProfiles),
                super::action("Import and merge", Message::ImportProfiles),
                act("Replace from file…", Action::ReplaceProfiles)
            ]
            .spacing(6),
            text(format!("Startup: {:?}", self.tools.startup.behavior)).size(12),
            row![
                act("Start with no session", Action::StartupNone),
                act(
                    "Load workspace metadata on startup",
                    Action::StartupWorkspace
                )
            ]
            .spacing(6)
        ]
        .spacing(10)
        .into()
    }
    fn workspace_tool_view(&self) -> Element<'_, Message> {
        let mut list = column![].spacing(5);
        for tab in &self.tabs {
            let id = Self::workspace_id(tab);
            if !self.tools.tab_query.is_empty()
                && !tab
                    .profile
                    .name
                    .to_lowercase()
                    .contains(&self.tools.tab_query.to_lowercase())
            {
                continue;
            }
            list = list.push(
                row![
                    checkbox(self.tools.selected_tabs.contains(&id))
                        .on_toggle(move |v| msg(Action::TabToggle(id, v))),
                    act(tab.profile.name.clone(), Action::TabSelect(id))
                ]
                .spacing(6),
            );
        }
        let mut colors = row![].spacing(5);
        for label in TabVisualLabel::ALL {
            colors = colors.push(act(label.name(), Action::TabLabel(label)));
        }
        let mut targets = column![text(
            "Synchronized input targets · changing targets disarms broadcast"
        )]
        .spacing(4);
        if let Some(tab) = self.tabs.get(self.active) {
            for (_, pane) in tab.panes.iter() {
                let id = pane.id;
                targets = targets.push(
                    checkbox(self.tools.sync.is_target(id))
                        .label(format!("{} · pane {id}", pane.profile.name))
                        .on_toggle(move |v| msg(Action::SyncTarget(id, v))),
                );
            }
        }
        let anchor = self
            .tabs
            .get(self.active)
            .map(Self::workspace_id)
            .unwrap_or_default();
        column![
            text("Tabs and workspace").size(24),
            input("Find tabs", &self.tools.tab_query, Field::TabQuery),
            scrollable(list).height(130),
            row![
                act("Move left", Action::TabMove(-1)),
                act("Move right", Action::TabMove(1)),
                act(
                    "Close selected…",
                    Action::CloseTabs(BulkCloseMode::Selected)
                ),
                act(
                    "Close right…",
                    Action::CloseTabs(BulkCloseMode::RightOf(anchor))
                ),
                act(
                    "Close others…",
                    Action::CloseTabs(BulkCloseMode::Others(anchor))
                )
            ]
            .spacing(5),
            colors,
            input(
                "Workspace layout path",
                &self.tools.workspace_path,
                Field::WorkspacePath
            ),
            checkbox(self.tools.workspace_reconnect)
                .label("Permit reconnect on explicit restore")
                .on_toggle(|v| msg(Action::WorkspaceReconnect(v))),
            row![
                act("Save workspace", Action::SaveWorkspace),
                act("Load metadata", Action::LoadWorkspace),
                act("Restore and reconnect", Action::RestoreWorkspace)
            ]
            .spacing(6),
            targets,
            checkbox(self.tools.sync.armed())
                .label("ARM synchronized input")
                .on_toggle(|v| msg(Action::SyncArm(v)))
        ]
        .spacing(10)
        .into()
    }
    fn history_tool_view(&self) -> Element<'_, Message> {
        let state = self
            .tools
            .history_owner
            .and_then(|id| self.tools.histories.get(&id));
        let mut results = column![].spacing(4);
        if let Some(state) = state {
            for (index, hit) in state
                .hits()
                .iter()
                .enumerate()
                .take(self.tools.history_visible)
            {
                results = results.push(act(
                    format!("{}:{} {}", hit.line_number, hit.column, hit.preview),
                    Action::HistorySelect(index),
                ));
            }
        }
        let mut marks = column![].spacing(4);
        if let Some(state) = state {
            for mark in state.marks().take(self.tools.history_visible) {
                marks = marks.push(
                    row![
                        text(format!(
                            "Mark {} · {:?}",
                            mark.line_id, mark.timestamp_epoch_seconds
                        )),
                        act(
                            if mark.collapsed { "Expand" } else { "Fold" },
                            Action::HistoryFold(mark.line_id)
                        ),
                        act("Remove", Action::HistoryRemove(mark.line_id))
                    ]
                    .spacing(6),
                );
            }
        }
        let mut lines = column![].spacing(2);
        if let Some(state) = state {
            for row in state
                .display_rows()
                .into_iter()
                .take(self.tools.history_visible)
            {
                lines = lines.push(
                    text(match row {
                        HistoryRow::Line(l) => l.text,
                        HistoryRow::Folded { hidden_lines, .. } => {
                            format!("[… {hidden_lines} folded lines …]")
                        }
                    })
                    .font(Font::MONOSPACE)
                    .size(12),
                );
            }
        }
        let logging = self
            .tools
            .history_owner
            .is_some_and(|id| self.tools.logs.contains_key(&id));
        column![
            text("Retained terminal history").size(24),
            text_input("Find text", state.map(|s| s.query()).unwrap_or(""))
                .on_input(|q| msg(Action::HistoryQuery(q)))
                .padding(8),
            row![
                act("Previous", Action::HistoryNext(false)),
                act("Next", Action::HistoryNext(true)),
                act("Mark output boundary", Action::HistoryMark),
                checkbox(self.tools.timestamps)
                    .label("Timestamp new marks")
                    .on_toggle(|v| msg(Action::HistoryTimestamps(v)))
            ]
            .spacing(6),
            scrollable(results).height(100),
            scrollable(marks).height(80),
            scrollable(lines).height(180),
            act("Show more history", Action::HistoryMore),
            text("Session logs record changed screen output; remote output may contain secrets.")
                .size(12),
            input("New session log path", &self.tools.log_path, Field::LogPath),
            act(
                if logging {
                    "Stop logging"
                } else {
                    "Start logging"
                },
                if logging {
                    Action::LogStop
                } else {
                    Action::LogStart
                }
            )
        ]
        .spacing(8)
        .into()
    }
    fn appearance_tool_view(&self) -> Element<'_, Message> {
        let a = &self.tools.appearance_draft;
        let mut fonts = row![text("Font family")].spacing(5);
        for (label, family) in [
            ("Monospace", TerminalFontFamily::Monospace),
            ("Proportional", TerminalFontFamily::Proportional),
        ] {
            fonts = fonts.push(act(label, Action::FontFamily(family)));
        }
        column![
            text("Terminal appearance and interaction").size(24),
            fonts,
            text(format!("Font size: {:.0}", a.font_size)),
            slider(8.0..=36.0, a.font_size, |v| msg(Action::FontSize(v))),
            row![
                act("Dark", Action::Palette(TerminalPalette::DefaultDark)),
                act("Light", Action::Palette(TerminalPalette::Light)),
                act(
                    "High contrast",
                    Action::Palette(TerminalPalette::HighContrast)
                )
            ]
            .spacing(5),
            input(
                "Foreground #RRGGBB (blank inherits palette)",
                a.foreground.as_deref().unwrap_or(""),
                Field::Foreground
            ),
            input(
                "Background #RRGGBB (blank inherits palette)",
                a.background.as_deref().unwrap_or(""),
                Field::Background
            ),
            text(a.contrast_warning().unwrap_or_default())
                .color(DANGER)
                .size(12),
            row![
                act("Block cursor", Action::Cursor(TerminalCursorStyle::Block)),
                act("Underline", Action::Cursor(TerminalCursorStyle::Underline)),
                act("Beam", Action::Cursor(TerminalCursorStyle::Beam))
            ]
            .spacing(5),
            text("Terminal surface opacity"),
            slider(0.35..=1.0, a.opacity, |v| msg(Action::Opacity(v))),
            checkbox(a.select_to_copy)
                .label("Select to copy")
                .on_toggle(|v| msg(Action::Pointer(0, v))),
            checkbox(a.middle_click_paste)
                .label("Middle-click paste")
                .on_toggle(|v| msg(Action::Pointer(1, v))),
            checkbox(a.right_click_paste)
                .label("Right-click paste")
                .on_toggle(|v| msg(Action::Pointer(2, v))),
            checkbox(a.hide_pointer_while_typing)
                .label("Hide pointer while typing")
                .on_toggle(|v| msg(Action::Pointer(3, v))),
            row![
                act("Save global", Action::AppearanceSave(false)),
                act("Save for this profile", Action::AppearanceSave(true)),
                act("Reset global", Action::AppearanceReset(false)),
                act("Clear profile override", Action::AppearanceReset(true))
            ]
            .spacing(5),
            row![
                act(
                    "Confirm multiline paste",
                    Action::PastePolicy(PastePolicy::ConfirmMultiline)
                )
                .style(if self.paste_policy == PastePolicy::ConfirmMultiline {
                    selected_button
                } else {
                    quiet
                }),
                act(
                    "Confirm every paste",
                    Action::PastePolicy(PastePolicy::ConfirmAll)
                )
                .style(if self.paste_policy == PastePolicy::ConfirmAll {
                    selected_button
                } else {
                    quiet
                }),
                act(
                    "Block multiline",
                    Action::PastePolicy(PastePolicy::BlockMultiline)
                )
            ]
            .spacing(5)
        ]
        .spacing(8)
        .into()
    }
    fn snippets_tool_view(&self) -> Element<'_, Message> {
        column![
            text("Snippet exchange and command targets").size(24),
            input(
                "Snippet JSON import/export path",
                &self.tools.snippet_path,
                Field::SnippetPath
            ),
            row![
                act("Import validated snippets", Action::ImportSnippets),
                act("Export snippets", Action::ExportSnippets)
            ]
            .spacing(6),
            input(
                "Complete snippet name",
                &self.tools.completion,
                Field::Completion
            ),
            act("Stage unique completion", Action::CompleteSnippet),
            text_editor(&self.command_content)
                .placeholder("Remote text to send")
                .on_action(Message::CommandEdit)
                .height(140)
                .padding(8),
            checkbox(self.tools.sender_sync)
                .label("Send to selected Workspace sync targets")
                .on_toggle(|v| msg(Action::SenderSync(v))),
            act("Preview and send…", Action::SendCommand),
            super::action("Manage snippets / command palette", Message::Commands)
        ]
        .spacing(10)
        .into()
    }
}

fn capture_workspace_tree(
    node: &pane_grid::Node,
    state: &pane_grid::State<TerminalPane>,
    profiles: &mut Vec<Session>,
) -> workspace::WorkspaceNode {
    use workspace::WorkspaceNode;
    match node {
        pane_grid::Node::Pane(pane) => {
            let index = profiles.len();
            profiles.push(
                state
                    .get(*pane)
                    .expect("live workspace pane")
                    .profile
                    .clone(),
            );
            WorkspaceNode::Pane { index }
        }
        pane_grid::Node::Split {
            axis, ratio, a, b, ..
        } => WorkspaceNode::Split {
            axis: match axis {
                pane_grid::Axis::Horizontal => workspace::SplitAxis::Horizontal,
                pane_grid::Axis::Vertical => workspace::SplitAxis::Vertical,
            },
            ratio: ratio.clamp(0.05, 0.95),
            a: Box::new(capture_workspace_tree(a, state, profiles)),
            b: Box::new(capture_workspace_tree(b, state, profiles)),
        },
    }
}
fn fallback_workspace_tree(count: usize, axis: workspace::SplitAxis) -> workspace::WorkspaceNode {
    fn node(index: usize, count: usize, axis: workspace::SplitAxis) -> workspace::WorkspaceNode {
        if index + 1 == count {
            workspace::WorkspaceNode::Pane { index }
        } else {
            workspace::WorkspaceNode::Split {
                axis,
                ratio: 0.5,
                a: Box::new(workspace::WorkspaceNode::Pane { index }),
                b: Box::new(node(index + 1, count, axis)),
            }
        }
    }
    node(0, count, axis)
}
fn restore_workspace_tree(
    node: &workspace::WorkspaceNode,
    panes: &mut [Option<TerminalPane>],
) -> pane_grid::Configuration<TerminalPane> {
    match node {
        workspace::WorkspaceNode::Pane { index } => pane_grid::Configuration::Pane(
            panes[*index].take().expect("validated unique pane index"),
        ),
        workspace::WorkspaceNode::Split { axis, ratio, a, b } => pane_grid::Configuration::Split {
            axis: match axis {
                workspace::SplitAxis::Horizontal => pane_grid::Axis::Horizontal,
                workspace::SplitAxis::Vertical => pane_grid::Axis::Vertical,
            },
            ratio: *ratio,
            a: Box::new(restore_workspace_tree(a, panes)),
            b: Box::new(restore_workspace_tree(b, panes)),
        },
    }
}

fn snapshot_text(snapshot: &terminal_core::DisplaySnapshot) -> String {
    let mut output = String::new();
    for row in 0..snapshot.rows {
        let mut column = 0;
        if let Some(&(start, end)) = snapshot.row_ranges.get(row) {
            for cell in &snapshot.cells[start..end] {
                while column < cell.column {
                    output.push(' ');
                    column += 1;
                }
                output.push(if cell.character == '\0' {
                    ' '
                } else {
                    cell.character
                });
                column = cell.column + if cell.wide { 2 } else { 1 };
            }
        }
        while output.ends_with(' ') {
            output.pop();
        }
        output.push('\n');
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app() -> (App, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        (App::boot(dir.path().join("sessions.json"), None), dir)
    }
    fn session(name: &str) -> Session {
        Session {
            name: name.into(),
            host: format!("{name}.example.invalid"),
            ..Session::default()
        }
    }
    fn pending_workspace(app: &mut App, name: &str) {
        let profile = session(name);
        let pane = app.new_terminal_pane(profile.clone());
        app.tabs.push(Workspace::new(profile, pane));
    }
    #[test]
    fn multiline_command_editor_stages_enter_without_sending() {
        let (mut app, _dir) = app();
        let _ = app.update(Message::CommandEdit(text_editor::Action::Edit(
            text_editor::Edit::Paste(Arc::new("printf one".into())),
        )));
        let _ = app.update(Message::CommandEdit(text_editor::Action::Edit(
            text_editor::Edit::Enter,
        )));
        let _ = app.update(Message::CommandEdit(text_editor::Action::Edit(
            text_editor::Edit::Paste(Arc::new("printf two".into())),
        )));
        assert_eq!(
            app.command_sender.lines().collect::<Vec<_>>(),
            vec!["printf one", "printf two"]
        );
        assert!(app.tabs.is_empty());
        assert!(app.tools.confirm.is_none());
        assert!(app.dialog.is_none());
    }
    #[test]
    fn snippet_editor_renames_existing_snippet_and_preserves_multiline_body() {
        let (mut app, _dir) = app();
        app.snippets.snippets.push(Snippet {
            name: "old".into(),
            body: "first\nsecond".into(),
        });
        let _ = app.update(Message::EditSnippet("old".into()));
        assert_eq!(app.snippet_content.text(), "first\nsecond");
        let _ = app.update(Message::SnippetName("renamed".into()));
        let _ = app.update(Message::SaveSnippet);
        assert_eq!(app.snippets.snippets.len(), 1);
        assert_eq!(app.snippets.snippets[0].name, "renamed");
        assert_eq!(
            command_palette::load_library(&app.snippets_path)
                .unwrap()
                .snippets[0]
                .body,
            "first\nsecond"
        );
        assert!(app.editing_snippet.is_none());
        assert!(app.command_sender.is_empty());
    }
    #[test]
    fn all_tool_panels_construct_without_a_connection() {
        let (mut app, _dir) = app();
        for panel in Panel::ALL {
            let _ = app.tool_update(Action::Open(panel));
            assert!(matches!(app.dialog, Some(Dialog::Tools)));
            let _view = app.tools_view();
            assert!(app.tabs.is_empty());
        }
    }
    #[test]
    fn four_pane_workspace_preserves_mixed_splits_ratios_and_profile_order() {
        let (mut app, _dir) = app();
        pending_workspace(&mut app, "first");
        for (name, axis) in [
            ("second", pane_grid::Axis::Vertical),
            ("third", pane_grid::Axis::Horizontal),
            ("fourth", pane_grid::Axis::Vertical),
        ] {
            let pane = app.new_terminal_pane(session(name));
            app.tabs[0].split(axis, pane);
        }
        let mut profiles = Vec::new();
        let tree = capture_workspace_tree(
            app.tabs[0].panes.layout(),
            &app.tabs[0].panes,
            &mut profiles,
        );
        let layout = WorkspaceLayout {
            panes: profiles.clone(),
            tree: Some(tree.clone()),
            reconnect_on_restore: true,
            ..WorkspaceLayout::default()
        };
        layout.validate().unwrap();
        app.tools.workspace = Some(layout);
        let _ = app.tool_update(Action::RestoreWorkspace);
        assert_eq!(app.tabs.len(), 2);
        assert_eq!(app.tabs[1].panes.len(), 4);
        let mut restored = Vec::new();
        let restored_tree = capture_workspace_tree(
            app.tabs[1].panes.layout(),
            &app.tabs[1].panes,
            &mut restored,
        );
        assert_eq!(tree, restored_tree);
        assert_eq!(profiles, restored);
        assert!(app.tabs[1].panes.iter().all(|(_, p)| p.terminal.is_none()));
    }
    #[test]
    fn loading_startup_workspace_never_connects_and_restore_requires_opt_in() {
        let (mut app, dir) = app();
        let path = dir.path().join("workspace.json");
        let layout = WorkspaceLayout {
            panes: vec![session("first")],
            ..WorkspaceLayout::default()
        };
        workspace::save_layout(&path, &layout).unwrap();
        startup::save_settings(
            &startup::settings_path(&app.profiles_path),
            &StartupSettings {
                behavior: StartupBehavior::Workspace {
                    path: path.display().to_string(),
                },
            },
        )
        .unwrap();
        app = App::boot(app.profiles_path.clone(), None);
        assert!(app.tools.workspace.is_some());
        app.start_configured_profile();
        assert!(app.tabs.is_empty());
        let _ = app.tool_update(Action::RestoreWorkspace);
        assert!(app.tabs.is_empty());
        assert!(app.status.contains("does not permit reconnect"));
    }
    #[test]
    fn sftp_reconnect_keeps_workspace_identity_and_sftp_launch_kind() {
        let (mut app, _dir) = app();
        let profile = session("files");
        let pane = app.new_terminal_pane_kind(profile.clone(), true);
        let old_id = pane.id;
        app.tabs.push(Workspace::new(profile, pane));
        let focus = app.tabs[0].focus;
        let workspace_id = app.tabs[0].id;
        app.tools
            .tab_labels
            .insert(workspace_id, TabVisualLabel::Red);
        let _ = app.update(Message::Reconnect(focus));
        let pane = app.tabs[0].panes.get(focus).unwrap();
        assert!(pane.sftp);
        assert_ne!(pane.id, old_id);
        assert_eq!(app.tabs[0].id, workspace_id);
        assert_eq!(app.tools.tab_labels[&workspace_id], TabVisualLabel::Red);
    }
    #[test]
    fn appearance_resize_changes_grid_and_selection_cell_geometry() {
        let (mut app, _dir) = app();
        pending_workspace(&mut app, "first");
        let id = app.tabs[0].id;
        app.resize_terminal(id, iced::Size::new(868.0, 568.0));
        let old = app.tabs[0]
            .panes
            .get(app.tabs[0].focus)
            .unwrap()
            .terminal_grid_size
            .unwrap();
        app.tools.appearance.global.font_size *= 2.0;
        app.invalidate_terminal_displays();
        let pane = app.tabs[0].panes.get(app.tabs[0].focus).unwrap();
        let new = pane.terminal_grid_size.unwrap();
        assert_eq!(new, (old.0 / 2, old.1 / 2));
        let appearance = app.tools.appearance.global.clone();
        let cell = terminal_cell_size(&appearance);
        let snapshot = terminal_core::DisplaySnapshot {
            rows: 0,
            columns: 0,
            background: [0; 3],
            row_ranges: vec![],
            cells: vec![],
        };
        let canvas = TerminalCanvas {
            focused: false,
            pane: app.tabs[0].focus,
            id,
            generation: 0,
            terminal_mode: terminal_core::TerminalMode::empty(),
            snapshot: &snapshot,
            appearance,
        };
        assert_eq!(
            canvas.cell_at(iced::Point::new(cell.0 + 0.1, cell.1 + 0.1)),
            (1, 1)
        );
    }
    #[test]
    fn paste_confirmation_cancels_when_broadcast_targets_change() {
        let (mut app, _dir) = app();
        pending_workspace(&mut app, "first");
        let second = app.new_terminal_pane(session("second"));
        app.tabs[0].split(pane_grid::Axis::Vertical, second);
        let source = app.tabs[0].panes.get(app.tabs[0].focus).unwrap().id;
        let ids: Vec<_> = app.tabs[0].panes.iter().map(|(_, p)| p.id).collect();
        for id in ids {
            app.tools.sync.set_target(id, true);
        }
        app.tools.sync.set_armed(true);
        let targets = app.paste_targets(source);
        assert_eq!(targets.len(), 2);
        app.dialog = Some(Dialog::PasteConfirm {
            id: source,
            text: "one\ntwo".into(),
            targets,
        });
        app.tools.sync.set_armed(false);
        let _ = app.update(Message::ConfirmPaste);
        assert!(app.status.contains("cancelled"));
        assert!(app.dialog.is_none());
    }
    #[test]
    fn stale_profile_job_does_not_dismiss_an_unrelated_dialog() {
        let (mut app, _dir) = app();
        app.dialog = Some(Dialog::About);
        app.tools.generation = 3;
        app.tools.busy = true;
        let _ = app.tool_update(Action::JobDone(
            2,
            Ok(Data::Profiles(vec![session("stale")])),
        ));
        assert!(app.profiles.is_empty());
        assert!(app.tools.busy);
        let _ = app.tool_update(Action::JobDone(
            3,
            Ok(Data::Profiles(vec![session("saved")])),
        ));
        assert_eq!(app.profiles.len(), 1);
        assert!(matches!(app.dialog, Some(Dialog::About)));
    }
    #[test]
    fn stopped_log_ignores_late_completion_and_closed_panes_are_pruned() {
        let (mut app, dir) = app();
        pending_workspace(&mut app, "first");
        let id = app.tabs[0].id;
        let path = dir.path().join("session.log");
        app.tools.log_pending.insert(id);
        let _ = app.tool_update(Action::LogWritten(id, path.clone(), "late".into(), Ok(())));
        assert!(!app.tools.logs.contains_key(&id));
        assert!(!app.tools.log_pending.contains(&id));
        app.tools.logs.insert(id, (path, String::new()));
        app.tools.sync.set_target(id, true);
        app.tabs.clear();
        app.prune_tool_panes();
        assert!(app.tools.logs.is_empty());
        assert_eq!(app.tools.sync.selected_count(), 0);
    }
}
