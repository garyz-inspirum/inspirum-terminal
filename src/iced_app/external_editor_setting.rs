//! Edit > External Editor...: the persisted editor command used by
//! Filer "Open in External Editor". Empty means the OS default application.

use super::*;

pub(super) const EXTERNAL_EDITOR_INPUT_ID: &str = "external-editor";
pub(super) const EXTERNAL_EDITOR_HELP: &str = r#"Leave empty to use the system default. Example: "C:\Program Files\Notepad++\notepad++.exe" {file}"#;

impl App {
    pub(super) fn open_external_editor_dialog(&mut self) -> Task<Message> {
        self.dialog = Some(Dialog::ExternalEditor {
            input: self.external_editor.clone(),
            error: None,
        });
        operation::focus(EXTERNAL_EDITOR_INPUT_ID)
    }

    pub(super) fn external_editor_input(&mut self, value: String) {
        if let Some(Dialog::ExternalEditor { input, .. }) = self.dialog.as_mut() {
            *input = value;
        }
    }

    /// Validate, store and persist. A rejected command keeps the dialog open
    /// with the reason and changes neither the setting nor the saved file.
    pub(super) fn save_external_editor_setting(&mut self) -> Task<Message> {
        let Some(Dialog::ExternalEditor { input, error }) = self.dialog.as_mut() else {
            return Task::none();
        };
        let value = input.trim().to_owned();
        if let Err(parse_error) = external_edit::EditorCommand::parse(&value) {
            *error = Some(format!("The editor command is not valid: {parse_error:#}"));
            return Task::none();
        }
        self.dialog = None;
        self.status = if value.is_empty() {
            "External editor: system default.".into()
        } else {
            "External editor saved.".into()
        };
        self.external_editor = value;
        // A save failure replaces the status with its own message.
        self.save_ui_state();
        self.restore_find_focus()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setting_app(name: &str) -> (App, PathBuf) {
        let dir = std::env::temp_dir().join(format!(
            "inspirum-external-editor-{}-{name}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let mut app = App::boot(dir.join("nonexistent-profiles.json"), None);
        let state_path = dir.join("ui-state.json");
        app.ui_state_path = Some(state_path.clone());
        app.external_editor = "vim".into();
        app.save_ui_state();
        (app, state_path)
    }

    fn open_and_type(app: &mut App, value: &str) {
        let _ = app.update(Message::MenuCommand(menu::Command::ExternalEditor));
        assert!(
            matches!(&app.dialog, Some(Dialog::ExternalEditor { input, error: None }) if input == "vim")
        );
        let _ = app.update(Message::ExternalEditorInput(value.into()));
    }

    #[test]
    fn invalid_external_editor_is_rejected_and_nothing_changes() {
        let (mut app, state_path) = setting_app("invalid");
        let before = std::fs::read(&state_path).unwrap();
        open_and_type(&mut app, "\"abc");
        let _ = app.update(Message::ExternalEditorSave);
        match &app.dialog {
            Some(Dialog::ExternalEditor {
                input,
                error: Some(error),
            }) => {
                assert_eq!(input, "\"abc");
                assert!(error.starts_with("The editor command is not valid: "));
            }
            _ => panic!("dialog must stay open with the error"),
        }
        assert_eq!(app.external_editor, "vim");
        assert_eq!(std::fs::read(&state_path).unwrap(), before);
    }

    #[test]
    fn valid_external_editor_is_trimmed_stored_and_saved() {
        let (mut app, state_path) = setting_app("valid");
        open_and_type(&mut app, "  code --wait {file}  ");
        let _ = app.update(Message::ExternalEditorSave);
        assert!(app.dialog.is_none());
        assert_eq!(app.external_editor, "code --wait {file}");
        assert_eq!(app.status, "External editor saved.");
        assert_eq!(
            ui_state::load(&state_path).external_editor,
            "code --wait {file}"
        );
    }

    #[test]
    fn empty_external_editor_selects_system_default() {
        let (mut app, state_path) = setting_app("empty");
        open_and_type(&mut app, "   ");
        let _ = app.update(Message::ExternalEditorSave);
        assert!(app.dialog.is_none());
        assert!(app.external_editor.is_empty());
        assert_eq!(app.status, "External editor: system default.");
        assert!(ui_state::load(&state_path).external_editor.is_empty());
    }

    #[test]
    fn cancel_leaves_external_editor_unchanged() {
        let (mut app, state_path) = setting_app("cancel");
        let before = std::fs::read(&state_path).unwrap();
        open_and_type(&mut app, "nano {file}");
        let _ = app.update(Message::CloseDialog);
        assert!(app.dialog.is_none());
        assert_eq!(app.external_editor, "vim");
        assert_eq!(std::fs::read(&state_path).unwrap(), before);
    }

    #[test]
    fn external_editor_menu_command_is_blocked_by_privacy_lock() {
        let (mut app, _) = setting_app("locked");
        app.privacy_locked = true;
        let _ = app.update(Message::MenuCommand(menu::Command::ExternalEditor));
        assert!(app.dialog.is_none());
    }
}
