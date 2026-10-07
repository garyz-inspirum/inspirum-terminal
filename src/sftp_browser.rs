use crate::{
    Session,
    remote_edit::{RemoteEdit, SaveOutcome},
    sftp::{self, RemoteEntry, Transfer},
};
use eframe::egui;
use std::{
    path::{Path, PathBuf},
    time::Duration,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Direction {
    Upload,
    Download,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum JobState {
    Queued,
    Running,
    Completed,
    Cancelled,
    Failed,
}

struct TransferJob {
    direction: Direction,
    local: PathBuf,
    remote: String,
    overwrite: bool,
    expected_size: Option<u64>,
    total: Option<u64>,
    transferred: u64,
    state: JobState,
    error: String,
    transfer: Option<Transfer>,
    resume_path: Option<PathBuf>,
    resume_available: bool,
    resume_requested: bool,
}

impl TransferJob {
    fn label(&self) -> String {
        match self.direction {
            Direction::Upload => format!("Upload {}", self.local.display()),
            Direction::Download => format!("Download {}", self.remote),
        }
    }
}

#[derive(Clone)]
struct Conflict {
    direction: Direction,
    local: PathBuf,
    remote: String,
    expected_size: Option<u64>,
}

pub struct SftpBrowser {
    session: Session,
    config: Option<PathBuf>,
    local_path: String,
    remote_path: String,
    local_entries: Vec<PathBuf>,
    remote_entries: Vec<RemoteEntry>,
    selected_local: Option<PathBuf>,
    selected_remote: Option<RemoteEntry>,
    mkdir_name: String,
    rename_name: String,
    local_mkdir_name: String,
    local_rename_name: String,
    local_delete_confirm: bool,
    drop_upload_enabled: bool,
    conflict: Option<Conflict>,
    delete_confirm: bool,
    remote_editor: Option<RemoteEdit>,
    remote_editor_conflict: bool,
    remote_editor_discard_confirm: bool,
    jobs: Vec<TransferJob>,
    notice: String,
    error: String,
}

impl SftpBrowser {
    pub fn new(session: Session, config: Option<PathBuf>) -> anyhow::Result<Self> {
        let local = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let mut browser = Self {
            session,
            config,
            local_path: local.to_string_lossy().into_owned(),
            remote_path: ".".into(),
            local_entries: Vec::new(),
            remote_entries: Vec::new(),
            selected_local: None,
            selected_remote: None,
            mkdir_name: String::new(),
            rename_name: String::new(),
            local_mkdir_name: String::new(),
            local_rename_name: String::new(),
            local_delete_confirm: false,
            drop_upload_enabled: false,
            conflict: None,
            delete_confirm: false,
            remote_editor: None,
            remote_editor_conflict: false,
            remote_editor_discard_confirm: false,
            jobs: Vec::new(),
            notice: String::new(),
            error: String::new(),
        };
        browser.refresh_local()?;
        browser.refresh_remote()?;
        Ok(browser)
    }

    pub fn session_name(&self) -> &str {
        &self.session.name
    }

    fn refresh_local(&mut self) -> anyhow::Result<()> {
        let path = PathBuf::from(self.local_path.trim());
        anyhow::ensure!(path.is_dir(), "local path is not a directory");
        self.local_entries = sftp::local_entries(&path)?;
        self.selected_local = None;
        Ok(())
    }

    fn refresh_remote(&mut self) -> anyhow::Result<()> {
        self.remote_entries = sftp::list_remote(
            &self.session,
            self.config.as_deref(),
            self.remote_path.trim(),
        )?;
        self.selected_remote = None;
        Ok(())
    }

    fn remote_parent(path: &str) -> String {
        if path == "/" || path == "." || path.is_empty() {
            return path.to_owned();
        }
        let path = path.trim_end_matches('/');
        match path.rsplit_once('/') {
            Some(("", _)) => "/".into(),
            Some((parent, _)) if !parent.is_empty() => parent.into(),
            _ => ".".into(),
        }
    }

    fn queue_job(&mut self, conflict: Conflict, overwrite: bool) {
        let total = match conflict.direction {
            Direction::Upload => std::fs::metadata(&conflict.local).ok().map(|m| m.len()),
            Direction::Download => conflict.expected_size,
        };
        self.jobs.push(TransferJob {
            direction: conflict.direction,
            local: conflict.local,
            remote: conflict.remote,
            overwrite,
            expected_size: conflict.expected_size,
            total,
            transferred: 0,
            state: JobState::Queued,
            error: String::new(),
            transfer: None,
            resume_path: None,
            resume_available: false,
            resume_requested: false,
        });
        self.notice = "Transfer queued.".into();
    }

    fn request_download(&mut self) -> anyhow::Result<()> {
        let remote = self
            .selected_remote
            .clone()
            .ok_or_else(|| anyhow::anyhow!("select a remote file to download"))?;
        anyhow::ensure!(!remote.is_dir, "select a remote file, not a directory");
        let local = sftp::local_destination(Path::new(self.local_path.trim()), &remote.name)?;
        let pending = Conflict {
            direction: Direction::Download,
            local: local.clone(),
            remote: sftp::join_remote(self.remote_path.trim(), &remote.name),
            expected_size: remote.size,
        };
        if local.exists() {
            self.conflict = Some(pending);
        } else {
            self.queue_job(pending, false);
        }
        Ok(())
    }

    fn request_upload_path(&mut self, local: PathBuf) -> anyhow::Result<()> {
        anyhow::ensure!(local.is_file(), "upload source must be a regular file");
        let name = local
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| anyhow::anyhow!("local filename must be valid Unicode"))?
            .to_owned();
        sftp::validate_local_leaf(&name)?;
        let remote = sftp::join_remote(self.remote_path.trim(), &name);
        let pending = Conflict {
            direction: Direction::Upload,
            local,
            remote,
            expected_size: None,
        };
        if self.remote_entries.iter().any(|entry| entry.name == name) {
            self.conflict = Some(pending);
        } else {
            self.queue_job(pending, false);
        }
        Ok(())
    }

    fn request_upload(&mut self) -> anyhow::Result<()> {
        let local = self
            .selected_local
            .clone()
            .ok_or_else(|| anyhow::anyhow!("select a local file to upload"))?;
        self.request_upload_path(local)
    }

    fn handle_dropped_files(&mut self, ctx: &egui::Context) {
        if !self.drop_upload_enabled {
            return;
        }
        let dropped = ctx.input(|input| input.raw.dropped_files.clone());
        if dropped.is_empty() {
            return;
        }
        let mut queued = 0usize;
        for file in dropped {
            let Some(path) = file.path else {
                self.error = "Dropped item has no local filesystem path.".into();
                continue;
            };
            match self.request_upload_path(path) {
                Ok(()) => queued += 1,
                Err(error) => self.error = format!("Cannot queue dropped file: {error:#}"),
            }
        }
        if queued > 0 {
            self.notice = format!(
                "Queued {queued} dropped file(s) for SFTP session '{}'.",
                self.session.name
            );
        }
    }

    fn open_remote_editor(&mut self) -> anyhow::Result<()> {
        let selected = self
            .selected_remote
            .clone()
            .ok_or_else(|| anyhow::anyhow!("select a remote file to edit"))?;
        anyhow::ensure!(!selected.is_dir, "select a remote file, not a directory");
        let remote = sftp::join_remote(self.remote_path.trim(), &selected.name);
        let editor = RemoteEdit::open(
            &self.session,
            self.config.as_deref(),
            &remote,
            selected.size,
        )?;
        self.remote_editor = Some(editor);
        self.remote_editor_conflict = false;
        self.remote_editor_discard_confirm = false;
        self.notice = format!("Opened '{remote}' in the private internal editor.");
        Ok(())
    }

    fn save_remote_editor(&mut self, force_remote_change: bool) -> anyhow::Result<()> {
        let editor = self
            .remote_editor
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("remote editor is not open"))?;
        match editor.save(&self.session, self.config.as_deref(), force_remote_change)? {
            SaveOutcome::Saved => {
                self.remote_editor_conflict = false;
                self.notice = format!(
                    "Saved '{}' through staged SFTP replacement.",
                    editor.remote()
                );
                let _ = self.refresh_remote();
            }
            SaveOutcome::Conflict => {
                self.remote_editor_conflict = true;
                self.notice =
                    "Remote file changed since it was opened; explicit overwrite confirmation is required."
                        .into();
            }
        }
        Ok(())
    }

    fn render_remote_editor(&mut self, ui: &mut egui::Ui) {
        let Some(editor) = self.remote_editor.as_mut() else {
            return;
        };
        let remote = editor.remote().to_owned();
        let dirty = editor.is_dirty();
        let mut save = false;
        let mut close = false;

        ui.separator();
        ui.horizontal(|ui| {
            ui.strong(format!("EDIT · {remote}"));
            if dirty {
                ui.small("● modified");
            } else {
                ui.small("saved");
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .small_button("×")
                    .on_hover_text("Close editor; unsaved changes require confirmation")
                    .clicked()
                {
                    if dirty {
                        self.remote_editor_discard_confirm = true;
                    } else {
                        close = true;
                    }
                }
                if ui.small_button("Save").on_hover_text(
                    "Re-check remote contents, then stage and replace explicitly",
                ).clicked() {
                    save = true;
                }
            });
        });
        ui.add(
            egui::TextEdit::multiline(editor.text_mut())
                .desired_rows(18)
                .desired_width(f32::INFINITY)
                .font(egui::TextStyle::Monospace),
        );

        if save && let Err(error) = self.save_remote_editor(false) {
            self.error = format!("{error:#}");
        }

        if self.remote_editor_conflict {
            ui.group(|ui| {
                ui.strong("Remote file changed");
                ui.small(
                    "The server copy changed after this editor was opened. Overwrite is never automatic.",
                );
                ui.horizontal(|ui| {
                    if ui.button("Overwrite changed remote file").clicked()
                        && let Err(error) = self.save_remote_editor(true)
                    {
                        self.error = format!("{error:#}");
                    }
                    if ui.button("Keep editing").clicked() {
                        self.remote_editor_conflict = false;
                    }
                });
            });
        }

        if self.remote_editor_discard_confirm {
            ui.group(|ui| {
                ui.strong("Discard unsaved changes?");
                ui.small("Nothing will be uploaded and the private working copy will be deleted.");
                ui.horizontal(|ui| {
                    if ui.button("Discard").clicked() {
                        close = true;
                    }
                    if ui.button("Keep editing").clicked() {
                        self.remote_editor_discard_confirm = false;
                    }
                });
            });
        }

        if close {
            self.remote_editor = None;
            self.remote_editor_conflict = false;
            self.remote_editor_discard_confirm = false;
            self.notice = format!("Closed editor for '{remote}' without automatic upload.");
        }
    }

    fn start_job(&mut self, index: usize) {
        let (direction, local, remote, overwrite, expected_size, resume_requested, resume_path) = {
            let job = &self.jobs[index];
            (
                job.direction,
                job.local.clone(),
                job.remote.clone(),
                job.overwrite,
                job.expected_size,
                job.resume_requested,
                job.resume_path.clone(),
            )
        };
        let result = match (direction, resume_requested) {
            (Direction::Download, true) => {
                let partial = resume_path
                    .as_deref()
                    .ok_or_else(|| anyhow::anyhow!("resumable download partial is unavailable"));
                partial.and_then(|partial| {
                    sftp::start_download_resume(
                        &self.session,
                        self.config.as_deref(),
                        &remote,
                        &local,
                        partial,
                        overwrite,
                        expected_size,
                    )
                })
            }
            (Direction::Download, false) => sftp::start_download(
                &self.session,
                self.config.as_deref(),
                &remote,
                &local,
                overwrite,
                expected_size,
            ),
            (Direction::Upload, true) => {
                sftp::start_upload_resume(&self.session, self.config.as_deref(), &local, &remote)
            }
            (Direction::Upload, false) => {
                sftp::start_upload(&self.session, self.config.as_deref(), &local, &remote)
            }
        };
        let job = &mut self.jobs[index];
        match result {
            Ok(transfer) => {
                job.transfer = Some(transfer);
                job.state = JobState::Running;
                job.error.clear();
            }
            Err(error) => {
                job.state = JobState::Failed;
                job.error = format!("{error:#}");
            }
        }
    }

    fn tick_transfers(&mut self, ctx: &egui::Context) {
        if let Some(index) = self
            .jobs
            .iter()
            .position(|job| job.state == JobState::Running)
        {
            let outcome = {
                let job = &mut self.jobs[index];
                let Some(transfer) = job.transfer.as_mut() else {
                    job.state = JobState::Failed;
                    job.error = "transfer process is missing".into();
                    return;
                };
                job.transferred = transfer.transferred_bytes();
                transfer.poll()
            };
            match outcome {
                Ok(Some(())) => {
                    let job = &mut self.jobs[index];
                    job.transferred = job.total.unwrap_or(job.transferred);
                    job.transfer = None;
                    job.resume_path = None;
                    job.resume_available = false;
                    job.resume_requested = false;
                    job.state = JobState::Completed;
                    self.notice = format!("{} completed and verified.", job.label());
                    let _ = self.refresh_local();
                    let _ = self.refresh_remote();
                }
                Ok(None) => {
                    ctx.request_repaint_after(Duration::from_millis(200));
                    return;
                }
                Err(error) => {
                    let job = &mut self.jobs[index];
                    let mut message = format!("{error:#}");
                    if let Some(transfer) = job.transfer.as_mut() {
                        match transfer.preserve_partial() {
                            Ok(partial) => {
                                if partial.is_some() {
                                    job.resume_path = partial;
                                }
                            }
                            Err(preserve_error) => {
                                message.push_str(&format!(
                                    "; preserving resumable partial also failed: {preserve_error:#}"
                                ));
                            }
                        }
                    }
                    job.transfer = None;
                    job.resume_available =
                        job.direction == Direction::Upload || job.resume_path.is_some();
                    job.resume_requested = false;
                    job.state = JobState::Failed;
                    job.error = message;
                }
            }
        }

        if let Some(index) = self
            .jobs
            .iter()
            .position(|job| job.state == JobState::Queued)
        {
            self.start_job(index);
            ctx.request_repaint_after(Duration::from_millis(100));
        }
    }

    fn render_local(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.strong("LOCAL");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.small_button("↻").on_hover_text("Refresh local directory").clicked()
                    && let Err(error) = self.refresh_local()
                {
                    self.error = format!("{error:#}");
                }
                if ui.small_button("↑").on_hover_text("Parent directory").clicked()
                    && let Some(parent) = Path::new(self.local_path.trim()).parent()
                {
                    self.local_path = parent.to_string_lossy().into_owned();
                    if let Err(error) = self.refresh_local() {
                        self.error = format!("{error:#}");
                    }
                }
            });
        });
        ui.add(
            egui::TextEdit::singleline(&mut self.local_path)
                .desired_width(f32::INFINITY)
                .hint_text("Local path"),
        );

        egui::ScrollArea::vertical()
            .max_height(320.0)
            .show(ui, |ui| {
                let mut enter = None;
                for path in &self.local_entries {
                    let name = path
                        .file_name()
                        .map(|name| name.to_string_lossy())
                        .unwrap_or_default();
                    let label = if path.is_dir() {
                        format!("▸ {name}")
                    } else {
                        format!("  {name}")
                    };
                    let response =
                        ui.selectable_label(self.selected_local.as_ref() == Some(path), label);
                    if response.clicked() {
                        self.local_rename_name = path
                            .file_name()
                            .map(|name| name.to_string_lossy().into_owned())
                            .unwrap_or_default();
                        self.selected_local = Some(path.clone());
                        self.local_delete_confirm = false;
                    }
                    if response.double_clicked() && path.is_dir() {
                        enter = Some(path.clone());
                    }
                }
                if let Some(path) = enter {
                    self.local_path = path.to_string_lossy().into_owned();
                    if let Err(error) = self.refresh_local() {
                        self.error = format!("{error:#}");
                    }
                }
            });

        egui::CollapsingHeader::new("Local actions")
            .default_open(false)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label("New folder");
                    ui.text_edit_singleline(&mut self.local_mkdir_name);
                    if ui.button("Create").clicked() {
                        let result = sftp::mkdir_local(
                            Path::new(self.local_path.trim()),
                            self.local_mkdir_name.trim(),
                        );
                        match result {
                            Ok(_) => {
                                self.local_mkdir_name.clear();
                                self.error.clear();
                                let _ = self.refresh_local();
                            }
                            Err(error) => self.error = format!("{error:#}"),
                        }
                    }
                });

                ui.horizontal(|ui| {
                    ui.label("Rename");
                    ui.text_edit_singleline(&mut self.local_rename_name);
                    if ui.button("Apply").clicked() {
                        let result = (|| -> anyhow::Result<()> {
                            let selected = self
                                .selected_local
                                .clone()
                                .ok_or_else(|| anyhow::anyhow!("select a local entry to rename"))?;
                            sftp::rename_local(&selected, self.local_rename_name.trim())?;
                            self.local_rename_name.clear();
                            self.refresh_local()
                        })();
                        if let Err(error) = result {
                            self.error = format!("{error:#}");
                        }
                    }
                    if ui.button("Delete…").clicked() {
                        self.local_delete_confirm = self.selected_local.is_some();
                    }
                });
            });

        if self.local_delete_confirm
            && let Some(selected) = self.selected_local.clone()
        {
            ui.group(|ui| {
                ui.strong(format!(
                    "Delete local {:?}?",
                    selected.file_name().unwrap_or_default()
                ));
                ui.small("Only files and empty directories are deleted.");
                ui.horizontal(|ui| {
                    if ui.button("Delete").clicked() {
                        match sftp::delete_local(&selected) {
                            Ok(()) => {
                                self.local_delete_confirm = false;
                                self.error.clear();
                                let _ = self.refresh_local();
                            }
                            Err(error) => self.error = format!("{error:#}"),
                        }
                    }
                    if ui.button("Cancel").clicked() {
                        self.local_delete_confirm = false;
                    }
                });
            });
        }
    }

    fn render_remote(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.strong("REMOTE");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.small_button("↻").on_hover_text("Refresh remote directory").clicked()
                    && let Err(error) = self.refresh_remote()
                {
                    self.error = format!("{error:#}");
                }
                if ui.small_button("↑").on_hover_text("Parent directory").clicked() {
                    self.remote_path = Self::remote_parent(self.remote_path.trim());
                    if let Err(error) = self.refresh_remote() {
                        self.error = format!("{error:#}");
                    }
                }
            });
        });
        ui.add(
            egui::TextEdit::singleline(&mut self.remote_path)
                .desired_width(f32::INFINITY)
                .hint_text("Remote path"),
        );

        egui::ScrollArea::vertical()
            .max_height(320.0)
            .show(ui, |ui| {
                let mut enter = None;
                for entry in &self.remote_entries {
                    let label = if entry.is_dir {
                        format!("▸ {}", entry.name)
                    } else {
                        format!(
                            "  {}{}",
                            entry.name,
                            entry
                                .size
                                .map(|size| format!("    {size} B"))
                                .unwrap_or_default()
                        )
                    };
                    let response =
                        ui.selectable_label(self.selected_remote.as_ref() == Some(entry), label);
                    if response.clicked() {
                        self.rename_name = entry.name.clone();
                        self.selected_remote = Some(entry.clone());
                        self.delete_confirm = false;
                    }
                    if response.double_clicked() && entry.is_dir {
                        enter = Some(entry.name.clone());
                    }
                }
                if let Some(name) = enter {
                    self.remote_path = sftp::join_remote(self.remote_path.trim(), &name);
                    if let Err(error) = self.refresh_remote() {
                        self.error = format!("{error:#}");
                    }
                }
            });
    }

    fn render_remote_actions(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.strong("TRANSFER");
            if ui.small_button("Upload →").clicked()
                && let Err(error) = self.request_upload()
            {
                self.error = format!("{error:#}");
            }
            if ui.small_button("← Download").clicked()
                && let Err(error) = self.request_download()
            {
                self.error = format!("{error:#}");
            }
            if ui.small_button("Edit").on_hover_text("Open selected remote text file").clicked()
                && let Err(error) = self.open_remote_editor()
            {
                self.error = format!("{error:#}");
            }
        });

        egui::CollapsingHeader::new("Remote actions")
            .default_open(false)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label("New folder");
                    ui.text_edit_singleline(&mut self.mkdir_name);
                    if ui.button("Create").clicked() {
                        let path =
                            sftp::join_remote(self.remote_path.trim(), self.mkdir_name.trim());
                        match sftp::mkdir_remote(&self.session, self.config.as_deref(), &path) {
                            Ok(()) => {
                                self.mkdir_name.clear();
                                self.error.clear();
                                let _ = self.refresh_remote();
                            }
                            Err(error) => self.error = format!("{error:#}"),
                        }
                    }
                });

                ui.horizontal(|ui| {
                    ui.label("Rename");
                    ui.text_edit_singleline(&mut self.rename_name);
                    if ui.button("Apply").clicked() {
                        let result = (|| -> anyhow::Result<()> {
                            let selected = self
                                .selected_remote
                                .clone()
                                .ok_or_else(|| anyhow::anyhow!("select a remote entry to rename"))?;
                            anyhow::ensure!(
                                !self.rename_name.trim().is_empty(),
                                "new name is required"
                            );
                            let from =
                                sftp::join_remote(self.remote_path.trim(), &selected.name);
                            let to =
                                sftp::join_remote(self.remote_path.trim(), self.rename_name.trim());
                            sftp::rename_remote(
                                &self.session,
                                self.config.as_deref(),
                                &from,
                                &to,
                            )?;
                            self.refresh_remote()
                        })();
                        if let Err(error) = result {
                            self.error = format!("{error:#}");
                        }
                    }
                    if ui.button("Delete…").clicked() {
                        self.delete_confirm = self.selected_remote.is_some();
                    }
                });
            });

        if self.delete_confirm
            && let Some(selected) = self.selected_remote.clone()
        {
            ui.group(|ui| {
                ui.strong(format!("Delete remote {:?}?", selected.name));
                ui.horizontal(|ui| {
                    if ui.button("Delete").clicked() {
                        let path = sftp::join_remote(self.remote_path.trim(), &selected.name);
                        match sftp::delete_remote(
                            &self.session,
                            self.config.as_deref(),
                            &path,
                            selected.is_dir,
                        ) {
                            Ok(()) => {
                                self.delete_confirm = false;
                                self.error.clear();
                                let _ = self.refresh_remote();
                            }
                            Err(error) => self.error = format!("{error:#}"),
                        }
                    }
                    if ui.button("Cancel").clicked() {
                        self.delete_confirm = false;
                    }
                });
            });
        }
    }

    fn render_conflict(&mut self, ui: &mut egui::Ui) {
        let Some(conflict) = self.conflict.clone() else {
            return;
        };
        ui.group(|ui| {
            let target = match conflict.direction {
                Direction::Download => conflict.local.display().to_string(),
                Direction::Upload => conflict.remote.clone(),
            };
            ui.label(format!("Target already exists: {target}"));
            ui.small(
                "Overwrite is never automatic. The existing target is changed only after you confirm.",
            );
            ui.horizontal(|ui| {
                if ui.button("Overwrite").clicked() {
                    self.queue_job(conflict.clone(), true);
                    self.conflict = None;
                }
                if ui.button("Cancel").clicked() {
                    self.conflict = None;
                }
            });
        });
    }

    fn render_queue(&mut self, ui: &mut egui::Ui) {
        ui.heading("Transfer queue");
        if self.jobs.is_empty() {
            ui.small("No transfers queued.");
            return;
        }
        for index in 0..self.jobs.len() {
            let mut cancel = false;
            let mut retry = false;
            let mut resume = false;
            let job = &self.jobs[index];
            ui.group(|ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label(job.label());
                    ui.strong(format!("{:?}", job.state));
                    if let Some(total) = job.total {
                        let done = if job.state == JobState::Completed {
                            total
                        } else {
                            job.transferred.min(total)
                        };
                        ui.label(format!("{done}/{total} bytes"));
                        if total > 0 {
                            ui.add(
                                egui::ProgressBar::new(done as f32 / total as f32)
                                    .desired_width(160.0),
                            );
                        }
                    }
                    if job.state == JobState::Running && ui.button("Cancel").clicked() {
                        cancel = true;
                    }
                    if matches!(job.state, JobState::Failed | JobState::Cancelled)
                        && ui.button("Retry").clicked()
                    {
                        retry = true;
                    }
                    if matches!(job.state, JobState::Failed | JobState::Cancelled)
                        && job.resume_available
                        && ui.button("Resume").clicked()
                    {
                        resume = true;
                    }
                });
                if !job.error.is_empty() {
                    ui.colored_label(egui::Color32::LIGHT_RED, &job.error);
                }
            });
            if cancel {
                let job = &mut self.jobs[index];
                if let Some(transfer) = job.transfer.as_mut() {
                    match transfer.cancel() {
                        Ok(partial) => {
                            if partial.is_some() {
                                job.resume_path = partial;
                            }
                        }
                        Err(error) => job.error = format!("{error:#}"),
                    }
                }
                job.transfer = None;
                job.resume_available =
                    job.direction == Direction::Upload || job.resume_path.is_some();
                job.resume_requested = false;
                job.state = JobState::Cancelled;
            }
            if retry {
                let job = &mut self.jobs[index];
                job.transfer = None;
                if let Some(partial) = job.resume_path.take() {
                    let _ = std::fs::remove_file(partial);
                }
                job.resume_available = false;
                job.resume_requested = false;
                job.state = JobState::Queued;
                job.transferred = 0;
                job.error.clear();
            }
            if resume {
                let job = &mut self.jobs[index];
                job.transfer = None;
                job.resume_requested = true;
                job.state = JobState::Queued;
                job.error.clear();
            }
        }
    }

    pub fn ui(&mut self, ctx: &egui::Context, ui: &mut egui::Ui) {
        self.tick_transfers(ctx);
        self.handle_dropped_files(ctx);

        ui.horizontal(|ui| {
            ui.strong(format!("SFTP · {}", self.session.name));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.menu_button("⋯", |ui| {
                    ui.checkbox(
                        &mut self.drop_upload_enabled,
                        "Accept dropped files for this session",
                    );
                    ui.small(
                        "Dropped files are ignored unless this session-specific target is enabled.",
                    );
                });
            });
        });
        ui.separator();

        ui.columns(2, |columns| {
            self.render_local(&mut columns[0]);
            self.render_remote(&mut columns[1]);
        });

        ui.separator();
        self.render_remote_actions(ui);
        self.render_remote_editor(ui);
        self.render_conflict(ui);

        if !self.notice.is_empty() {
            ui.small(&self.notice);
        }
        if !self.error.is_empty() {
            ui.colored_label(egui::Color32::LIGHT_RED, &self.error);
        }

        if !self.jobs.is_empty() {
            ui.separator();
            egui::CollapsingHeader::new(format!("Transfers ({})", self.jobs.len()))
                .default_open(true)
                .show(ui, |ui| self.render_queue(ui));
        }
    }

}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_parent_stays_bounded() {
        assert_eq!(SftpBrowser::remote_parent("/a/b"), "/a");
        assert_eq!(SftpBrowser::remote_parent("/a"), "/");
        assert_eq!(SftpBrowser::remote_parent("/"), "/");
        assert_eq!(SftpBrowser::remote_parent("a"), ".");
        assert_eq!(SftpBrowser::remote_parent("."), ".");
    }
}
