use crate::{Session, scp};
use eframe::egui;
use std::{
    path::PathBuf,
    time::Duration,
};

pub struct ScpPanel {
    session: Session,
    config: Option<PathBuf>,
    local_path: String,
    remote_path: String,
    overwrite: bool,
    transfer: Option<scp::Transfer>,
    completed: bool,
    cancelled: bool,
    error: String,
}

impl ScpPanel {
    pub fn new(session: Session, config: Option<PathBuf>) -> Self {
        Self {
            session,
            config,
            local_path: String::new(),
            remote_path: String::new(),
            overwrite: false,
            transfer: None,
            completed: false,
            cancelled: false,
            error: String::new(),
        }
    }

    pub fn session_name(&self) -> &str {
        &self.session.name
    }

    fn start_upload(&mut self) {
        let local = PathBuf::from(self.local_path.trim());
        match scp::start_upload(
            &self.session,
            self.config.as_deref(),
            &local,
            self.remote_path.trim(),
            self.overwrite,
        ) {
            Ok(transfer) => {
                self.transfer = Some(transfer);
                self.completed = false;
                self.cancelled = false;
                self.error.clear();
            }
            Err(error) => {
                self.error = format!("{error:#}");
            }
        }
    }

    fn start_download(&mut self) {
        let local = PathBuf::from(self.local_path.trim());
        match scp::start_download(
            &self.session,
            self.config.as_deref(),
            self.remote_path.trim(),
            &local,
            self.overwrite,
        ) {
            Ok(transfer) => {
                self.transfer = Some(transfer);
                self.completed = false;
                self.cancelled = false;
                self.error.clear();
            }
            Err(error) => {
                self.error = format!("{error:#}");
            }
        }
    }

    fn tick(&mut self, ctx: &egui::Context) {
        let Some(transfer) = self.transfer.as_mut() else {
            return;
        };
        match transfer.poll() {
            Ok(Some(())) => {
                self.transfer = None;
                self.completed = true;
                self.cancelled = false;
                self.error.clear();
            }
            Ok(None) => {
                ctx.request_repaint_after(Duration::from_millis(200));
            }
            Err(error) => {
                self.transfer = None;
                self.completed = false;
                self.cancelled = false;
                self.error = format!("{error:#}");
            }
        }
    }

    pub fn ui(&mut self, ctx: &egui::Context, ui: &mut egui::Ui) {
        self.tick(ctx);
        ui.heading(format!("SCP transfer · {}", self.session.name));
        ui.small(
            "Uses the system OpenSSH scp client with the same host-key, authentication, config, proxy and ControlMaster policy as this profile.",
        );
        ui.label("Local file");
        ui.text_edit_singleline(&mut self.local_path);
        ui.label("Remote file");
        ui.text_edit_singleline(&mut self.remote_path);
        ui.checkbox(
            &mut self.overwrite,
            "Allow overwrite if the destination already exists",
        );
        ui.small(
            "Overwrite is never implicit. Uploads are staged remotely and verified before the final rename; downloads are staged locally and verified before commit.",
        );

        let running = self.transfer.is_some();
        ui.horizontal(|ui| {
            if ui
                .add_enabled(!running, egui::Button::new("Upload with SCP"))
                .clicked()
            {
                self.start_upload();
            }
            if ui
                .add_enabled(!running, egui::Button::new("Download with SCP"))
                .clicked()
            {
                self.start_download();
            }
            if ui
                .add_enabled(running, egui::Button::new("Cancel"))
                .clicked()
                && let Some(mut transfer) = self.transfer.take()
            {
                match transfer.cancel() {
                    Ok(()) => {
                        self.completed = false;
                        self.cancelled = true;
                        self.error.clear();
                    }
                    Err(error) => self.error = format!("{error:#}"),
                }
            }
        });

        if let Some(transfer) = self.transfer.as_ref() {
            let total = transfer.total_bytes();
            let done = transfer.transferred_bytes().min(total);
            ui.horizontal(|ui| {
                ui.strong(match transfer.direction() {
                    scp::Direction::Upload => "Uploading",
                    scp::Direction::Download => "Downloading",
                });
                ui.label(format!("{done}/{total} bytes"));
                if total > 0 {
                    ui.add(
                        egui::ProgressBar::new(done as f32 / total as f32).desired_width(180.0),
                    );
                }
            });
            if transfer.direction() == scp::Direction::Upload {
                ui.small(
                    "OpenSSH scp does not expose reliable machine-readable per-byte upload progress without a terminal; the total is shown and completion is verified before success.",
                );
            }
        } else if self.completed {
            ui.strong("Transfer completed and verified.");
        } else if self.cancelled {
            ui.strong("Transfer cancelled; staging cleanup requested.");
        }

        if !self.error.is_empty() {
            ui.colored_label(egui::Color32::LIGHT_RED, &self.error);
        }
    }
}
