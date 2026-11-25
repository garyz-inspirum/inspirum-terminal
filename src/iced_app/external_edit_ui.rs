//! Filer "Open in External Editor": App-side wiring of `external_edit`.
//!
//! Opening and uploading run on a blocking worker (`run_blocking_result`);
//! the UI thread only `try_lock`s an edit for the cheap `poll_changed` stat
//! check, so it never waits on network I/O. Polling is a self-rescheduling
//! one-second sleep task that exists only while at least one edit is open.

use super::*;

const EXTERNAL_EDIT_POLL: Duration = Duration::from_secs(1);

impl App {
    pub(super) fn request_external_edit(&mut self) -> Task<Message> {
        let Some(entry) = self.files.selected_remote.clone() else {
            self.status = "Select a remote file to open in the external editor.".into();
            return Task::none();
        };
        if entry.is_dir {
            self.status = "Select a remote file, not a directory.".into();
            return Task::none();
        }
        let Some(session) = self.tabs.get(self.active).map(|tab| tab.profile.clone()) else {
            return Task::none();
        };
        let command = match external_edit::EditorCommand::parse(&self.external_editor) {
            Ok(command) => command,
            Err(error) => {
                self.status = format!("The external editor setting is not valid: {error:#}");
                return Task::none();
            }
        };
        let session_key = session_profile_key(&session);
        let remote = sftp::join_remote(&self.files.remote_dir, &entry.name);
        if let Some(existing) = self
            .external_edits
            .iter()
            .find(|edit| edit.session_key == session_key && edit.remote == remote)
        {
            // Already open: re-launch on the same working copy, no download.
            self.status = match external_edit::launch(&command, &existing.working_path) {
                Ok(()) => opened_status(&remote),
                Err(error) => format!("Could not start the external editor: {error:#}"),
            };
            return Task::none();
        }
        let expected_size = entry.size;
        let config = self.ssh_config.clone();
        let open_session = session.clone();
        let open_remote = remote.clone();
        self.status = format!("Downloading {remote} for the external editor...");
        Task::perform(
            run_blocking_result(move || {
                external_edit::ExternalEdit::open(
                    &open_session,
                    config.as_deref(),
                    &open_remote,
                    expected_size,
                )
                .map(|edit| ExternalEditHandle(Arc::new(Mutex::new(edit))))
                .map_err(|error| format!("{error:#}"))
            }),
            move |result| Message::ExternalEditOpened {
                session,
                session_key,
                remote,
                result,
            },
        )
    }

    pub(super) fn external_edit_opened(
        &mut self,
        session: Session,
        session_key: String,
        remote: String,
        result: Result<ExternalEditHandle, String>,
    ) -> Task<Message> {
        // Stale result: the tab that asked for it is gone. Dropping the
        // handle removes the private working copy; no other tab is touched.
        if !self
            .tabs
            .iter()
            .any(|tab| session_profile_key(&tab.profile) == session_key)
        {
            self.status = format!(
                "Finished downloading {remote}, but its SSH session was closed; nothing was opened."
            );
            return Task::none();
        }
        let handle = match result {
            Ok(handle) => handle,
            Err(error) => {
                self.status = format!("Could not open {remote} in the external editor: {error}");
                return Task::none();
            }
        };
        let command = match external_edit::EditorCommand::parse(&self.external_editor) {
            Ok(command) => command,
            Err(error) => {
                self.status = format!("The external editor setting is not valid: {error:#}");
                return Task::none();
            }
        };
        if let Some(existing) = self
            .external_edits
            .iter()
            .find(|edit| edit.session_key == session_key && edit.remote == remote)
        {
            // A second open raced the first: keep the existing working copy.
            self.status = match external_edit::launch(&command, &existing.working_path) {
                Ok(()) => opened_status(&remote),
                Err(error) => format!("Could not start the external editor: {error:#}"),
            };
            return Task::none();
        }
        let working_path = match handle.0.lock() {
            Ok(edit) => edit.working_path().to_path_buf(),
            Err(_) => {
                self.status = format!("Could not open {remote} in the external editor.");
                return Task::none();
            }
        };
        if let Err(error) = external_edit::launch(&command, &working_path) {
            self.status = format!("Could not start the external editor: {error:#}");
            return Task::none();
        }
        let id = self.next_external_edit_id;
        self.next_external_edit_id = self.next_external_edit_id.saturating_add(1);
        self.external_edits.push(ExternalEditState {
            id,
            session,
            session_key,
            remote: remote.clone(),
            working_path,
            handle,
            uploading: false,
            unsaved: false,
        });
        self.status = opened_status(&remote);
        self.schedule_external_edit_tick()
    }

    fn schedule_external_edit_tick(&mut self) -> Task<Message> {
        if self.external_edit_tick_scheduled || self.external_edits.is_empty() {
            return Task::none();
        }
        self.external_edit_tick_scheduled = true;
        Task::perform(
            run_blocking_result(|| {
                thread::sleep(EXTERNAL_EDIT_POLL);
                Ok(())
            }),
            |_: Result<(), String>| Message::ExternalEditTick,
        )
    }

    pub(super) fn external_edit_tick(&mut self) -> Task<Message> {
        self.external_edit_tick_scheduled = false;
        let mut tasks = Vec::new();
        let config = self.ssh_config.clone();
        for edit in &mut self.external_edits {
            if edit.uploading {
                continue;
            }
            // An upload holds the lock; skip this tick rather than block.
            let changed = match edit.handle.0.try_lock() {
                Ok(mut inner) => inner.poll_changed(),
                Err(_) => continue,
            };
            match changed {
                Ok(Some(_)) => {
                    edit.uploading = true;
                    let id = edit.id;
                    let handle = edit.handle.clone();
                    let session = edit.session.clone();
                    let config = config.clone();
                    tasks.push(Task::perform(
                        run_blocking_result(move || {
                            let mut inner = handle.0.lock().map_err(|_| {
                                "The external edit is no longer usable.".to_string()
                            })?;
                            inner
                                .upload(&session, config.as_deref(), false)
                                .map_err(|error| format!("{error:#}"))
                        }),
                        move |result| Message::ExternalEditUploaded { id, result },
                    ));
                }
                Ok(None) => {}
                Err(error) => {
                    self.status = format!("Could not check {} for changes: {error:#}", edit.remote);
                }
            }
        }
        tasks.push(self.schedule_external_edit_tick());
        Task::batch(tasks)
    }

    pub(super) fn external_edit_uploaded(
        &mut self,
        id: u64,
        result: Result<external_edit::UploadOutcome, String>,
    ) {
        // The edit was closed meanwhile: nothing to report or update.
        let Some(edit) = self.external_edits.iter_mut().find(|edit| edit.id == id) else {
            return;
        };
        edit.uploading = false;
        let remote = edit.remote.clone();
        let kept = edit.working_path.display().to_string();
        match result {
            Ok(external_edit::UploadOutcome::Uploaded) => {
                edit.unsaved = false;
                self.status = format!("Uploaded {remote}.");
            }
            Ok(external_edit::UploadOutcome::RemoteChangedConflict) => {
                edit.unsaved = true;
                self.status = format!(
                    "{remote} changed on the server; your edit was NOT uploaded. The local copy is kept at {kept}."
                );
            }
            Ok(external_edit::UploadOutcome::Unchanged) => edit.unsaved = false,
            Ok(external_edit::UploadOutcome::WorkingCopyMissing) => {}
            Err(error) => {
                edit.unsaved = true;
                self.status = format!(
                    "Could not upload {remote}: {error}. The local copy is kept at {kept}."
                );
            }
        }
        // Its tab may have closed while the upload ran.
        self.close_orphaned_external_edits();
    }

    /// Close edits whose profile has no open tab left. An edit is deleted
    /// only when its working copy is established as byte-equal to the last
    /// upload (`ExternalEditState::is_established_synced`). Anything else
    /// (uploading, a failed upload, a save the two-poll debounce has not
    /// picked up yet, a missing or unreadable copy) stays in
    /// `external_edits` with `unsaved` set: later calls keep it for the same
    /// reason, the poller can still upload it, and exit leaks its directory.
    pub(super) fn close_orphaned_external_edits(&mut self) {
        let open_keys: Vec<String> = self
            .tabs
            .iter()
            .map(|tab| session_profile_key(&tab.profile))
            .collect();
        let mut kept = Vec::new();
        for mut edit in std::mem::take(&mut self.external_edits) {
            if open_keys.contains(&edit.session_key) || edit.uploading {
                kept.push(edit);
            } else if !edit.is_established_synced() {
                edit.unsaved = true;
                self.status = kept_copy_status(&edit);
                kept.push(edit);
            }
            // Otherwise dropping the state drops the last handle, which
            // removes the private working directory.
        }
        self.external_edits = kept;
    }

    /// On exit: copies established as synced are removed; every other copy
    /// is deliberately leaked so its directory survives the process. The
    /// status line is gone by then, so each kept path goes to stderr.
    pub(super) fn close_external_edits_for_exit(&mut self) {
        for edit in std::mem::take(&mut self.external_edits) {
            if !edit.is_established_synced() {
                eprintln!("{}", kept_copy_status(&edit));
                std::mem::forget(edit.handle);
            } else if let Ok(mutex) = Arc::try_unwrap(edit.handle.0)
                && let Ok(inner) = mutex.into_inner()
            {
                let _ = inner.close();
            }
        }
    }
}

impl ExternalEditState {
    /// True only when deleting the working copy cannot lose work: no upload
    /// running or failed, and the file on disk byte-equals the last synced
    /// content. A lock that cannot be taken counts as not established. This
    /// reads the local working copy only; no network I/O.
    fn is_established_synced(&self) -> bool {
        !self.unsaved
            && !self.uploading
            && self
                .handle
                .0
                .try_lock()
                .is_ok_and(|inner| inner.working_copy_is_synced())
    }
}

fn kept_copy_status(edit: &ExternalEditState) -> String {
    format!(
        "{} was not uploaded; the local copy is kept at {}.",
        edit.remote,
        edit.working_path.display()
    )
}

fn opened_status(remote: &str) -> String {
    format!("Opened {remote} in external editor. Saving in the editor uploads it.")
}
