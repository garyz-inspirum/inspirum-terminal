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
        };
        save(&path, &state).unwrap();
        assert_eq!(load(&path), state);
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
