//! Small file-backed store for chrome UI preferences (theme, dock layout).
//! Missing, corrupt or oversized files fall back to defaults.

use std::io::Read;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::dock::DockLayout;

const MAX_BYTES: u64 = 64 * 1024;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub(super) struct UiState {
    pub(super) theme: String,
    pub(super) docks: Option<DockLayout>,
    /// External editor command; empty means the OS default application.
    pub(super) external_editor: String,
}

pub(super) fn path_for(profiles_path: &Path) -> PathBuf {
    profiles_path.with_file_name("ui-state.json")
}

pub(super) fn load(path: &Path) -> UiState {
    let Ok(file) = std::fs::File::open(path) else {
        return UiState::default();
    };
    let mut bytes = Vec::new();
    if file.take(MAX_BYTES + 1).read_to_end(&mut bytes).is_err() || bytes.len() as u64 > MAX_BYTES {
        return UiState::default();
    }
    serde_json::from_slice(&bytes).unwrap_or_default()
}

pub(super) fn save(path: &Path, state: &UiState) -> std::io::Result<()> {
    let json = serde_json::to_vec_pretty(state).map_err(std::io::Error::other)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, json)?;
    std::fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("inspirum-ui-state-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("ui-state.json")
    }

    #[test]
    fn round_trip() {
        let path = temp_path("round");
        let state = UiState {
            theme: "dark".into(),
            docks: Some(DockLayout::default()),
            external_editor: String::new(),
        };
        save(&path, &state).unwrap();
        assert_eq!(load(&path), state);
    }

    #[test]
    fn external_editor_round_trips() {
        let path = temp_path("editor");
        let state = UiState {
            theme: "light".into(),
            docks: None,
            external_editor: r#""C:\Program Files\Notepad++\notepad++.exe" {file}"#.into(),
        };
        save(&path, &state).unwrap();
        assert_eq!(load(&path), state);
    }

    #[test]
    fn file_without_external_editor_loads_empty() {
        let path = temp_path("no-editor");
        std::fs::write(&path, br#"{"theme":"dark"}"#).unwrap();
        let state = load(&path);
        assert_eq!(state.theme, "dark");
        assert!(state.external_editor.is_empty());
    }

    #[test]
    fn corrupt_file_is_default() {
        let path = temp_path("corrupt");
        std::fs::write(&path, b"{not json").unwrap();
        assert_eq!(load(&path), UiState::default());
        assert_eq!(
            load(&path.with_file_name("missing.json")),
            UiState::default()
        );
    }

    #[test]
    fn oversized_file_is_default() {
        let path = temp_path("big");
        let big = format!(
            "{{\"theme\":\"dark\",\"pad\":\"{}\"}}",
            "x".repeat(70 * 1024)
        );
        std::fs::write(&path, big).unwrap();
        assert_eq!(load(&path), UiState::default());
    }
}
