//! Safe startup behavior for saved profiles and workspaces.
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{fs, io::Read, path::{Path, PathBuf}};

const MAX_STARTUP_BYTES: usize = 65_536;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
pub enum StartupBehavior {
    #[default]
    None,
    Profile { profile: String },
    /// Startup workspace handling is metadata-only. Reconnection still requires the
    /// existing explicit Restore & reconnect action and persisted reconnect permission.
    Workspace { path: String },
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StartupSettings {
    #[serde(default)]
    pub behavior: StartupBehavior,
}

impl StartupSettings {
    pub fn validate(&self) -> Result<()> {
        match &self.behavior {
            StartupBehavior::None => {}
            StartupBehavior::Profile { profile } => {
                ensure!(
                    !profile.is_empty() && profile.len() <= 1024 && !profile.chars().any(char::is_control),
                    "startup profile selector is invalid"
                );
            }
            StartupBehavior::Workspace { path } => {
                ensure!(
                    !path.trim().is_empty() && path.len() <= 4096 && !path.chars().any(char::is_control),
                    "startup workspace path is invalid"
                );
            }
        }
        Ok(())
    }
}

pub fn settings_path(profiles_path: &Path) -> PathBuf {
    let file_name = profiles_path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("sessions.json");
    profiles_path.with_file_name(format!("{file_name}.startup.json"))
}

pub fn load_settings(path: &Path) -> Result<StartupSettings> {
    let file = match fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(StartupSettings::default());
        }
        Err(error) => return Err(error).context("open startup settings"),
    };
    let mut bytes = Vec::new();
    file.take((MAX_STARTUP_BYTES + 1) as u64).read_to_end(&mut bytes)?;
    ensure!(bytes.len() <= MAX_STARTUP_BYTES, "startup settings exceed 64 KiB");
    let settings: StartupSettings =
        serde_json::from_slice(&bytes).context("parse startup settings")?;
    settings.validate()?;
    Ok(settings)
}

pub fn save_settings(path: &Path, settings: &StartupSettings) -> Result<()> {
    settings.validate()?;
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    fs::create_dir_all(parent).context("create startup settings directory")?;
    let mut bytes = serde_json::to_vec_pretty(settings)?;
    bytes.push(b'\n');
    ensure!(bytes.len() <= MAX_STARTUP_BYTES, "startup settings exceed 64 KiB");
    let mut temp =
        tempfile::NamedTempFile::new_in(parent).context("create startup settings temp file")?;
    use std::io::Write;
    temp.write_all(&bytes)?;
    temp.as_file().sync_all()?;
    temp.persist(path)
        .map_err(|error| error.error)
        .context("replace startup settings")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_settings_default_to_no_startup_action() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            load_settings(&dir.path().join("missing.json")).unwrap(),
            StartupSettings::default()
        );
    }

    #[test]
    fn round_trip_keeps_profile_and_workspace_choices_non_secret() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("startup.json");
        let settings = StartupSettings {
            behavior: StartupBehavior::Profile {
                profile: "prod\u{1f}router".into(),
            },
        };
        save_settings(&path, &settings).unwrap();
        assert_eq!(load_settings(&path).unwrap(), settings);

        let bytes = fs::read_to_string(path).unwrap();
        assert!(!bytes.to_lowercase().contains("password"));
        assert!(!bytes.to_lowercase().contains("passphrase"));
    }

    #[test]
    fn workspace_startup_is_only_a_path_choice() {
        let settings = StartupSettings {
            behavior: StartupBehavior::Workspace {
                path: "/tmp/layout.json".into(),
            },
        };
        settings.validate().unwrap();
    }
}
