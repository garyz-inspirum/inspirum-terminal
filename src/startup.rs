//! Safe startup behavior for saved profiles and workspaces.
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
};

const MAX_STARTUP_BYTES: usize = 65_536;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
pub enum StartupBehavior {
    #[default]
    None,
    Profile {
        profile: String,
    },
    /// Startup workspace handling is metadata-only. Reconnection still requires the
    /// existing explicit Restore & reconnect action and persisted reconnect permission.
    Workspace {
        path: String,
    },
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StartupSettings {
    #[serde(default)]
    pub behavior: StartupBehavior,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ssh_config: Option<String>,
}

impl StartupSettings {
    pub fn validate(&self) -> Result<()> {
        if let Some(path) = &self.ssh_config {
            validate_ssh_config_path(Path::new(path))?;
        }
        match &self.behavior {
            StartupBehavior::None => {}
            StartupBehavior::Profile { profile } => {
                ensure!(
                    !profile.is_empty()
                        && profile.len() <= 1024
                        && !profile.chars().any(char::is_control),
                    "startup profile selector is invalid"
                );
            }
            StartupBehavior::Workspace { path } => {
                ensure!(
                    !path.trim().is_empty()
                        && path.len() <= 4096
                        && !path.chars().any(char::is_control),
                    "startup workspace path is invalid"
                );
            }
        }
        Ok(())
    }
}

/// Validate the saved path. Save and launch also read from one opened
/// descriptor, so replacing the path after Save fails closed at launch.
pub fn validate_ssh_config_path(path: &Path) -> Result<()> {
    let value = path.to_str().context("SSH config path must be UTF-8")?;
    ensure!(!value.trim().is_empty(), "SSH config path is blank");
    ensure!(
        !value.chars().any(char::is_control),
        "SSH config path contains control characters"
    );
    ensure!(path.is_absolute(), "SSH config path must be absolute");
    ensure!(value.len() <= 4096, "SSH config path is too long");
    ensure!(
        fs::metadata(path)
            .context("inspect SSH config path")?
            .is_file(),
        "SSH config must be a regular file"
    );
    Ok(())
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
    file.take((MAX_STARTUP_BYTES + 1) as u64)
        .read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() <= MAX_STARTUP_BYTES,
        "startup settings exceed 64 KiB"
    );
    let settings: StartupSettings =
        serde_json::from_slice(&bytes).context("parse startup settings")?;
    settings.validate()?;
    Ok(settings)
}

/// Save a newly chosen SSH config path: its content is checked first.
pub fn save_settings_checking_config(path: &Path, settings: &StartupSettings) -> Result<()> {
    save_settings(path, settings)
}

pub fn save_settings(path: &Path, settings: &StartupSettings) -> Result<()> {
    settings.validate()?;
    if let Some(config) = &settings.ssh_config {
        crate::tunnels::read_config_bytes(Path::new(config))
            .context("read SSH config before saving startup settings")?;
    }
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    fs::create_dir_all(parent).context("create startup settings directory")?;
    let mut bytes = serde_json::to_vec_pretty(settings)?;
    bytes.push(b'\n');
    ensure!(
        bytes.len() <= MAX_STARTUP_BYTES,
        "startup settings exceed 64 KiB"
    );
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
                profile: "4:prodrouter".into(),
            },
            ssh_config: None,
        };
        save_settings(&path, &settings).unwrap();
        assert_eq!(load_settings(&path).unwrap(), settings);

        let bytes = fs::read_to_string(path).unwrap();
        assert!(!bytes.to_lowercase().contains("password"));
        assert!(!bytes.to_lowercase().contains("passphrase"));
    }

    #[test]
    fn saved_config_round_trip_and_behavior_updates_preserve_path() {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("config");
        fs::write(&config, "Host *\n  ForwardAgent no\n").unwrap();
        let path = dir.path().join("startup.json");
        let mut settings = StartupSettings {
            ssh_config: Some(config.to_str().unwrap().into()),
            ..Default::default()
        };
        save_settings(&path, &settings).unwrap();
        settings.behavior = StartupBehavior::Profile {
            profile: "prod".into(),
        };
        save_settings(&path, &settings).unwrap();
        assert_eq!(load_settings(&path).unwrap(), settings);
    }

    #[test]
    fn saved_config_rejects_non_regular_and_relative_paths_without_writing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("startup.json");
        let settings = StartupSettings {
            ssh_config: Some("relative".into()),
            ..Default::default()
        };
        assert!(save_settings(&path, &settings).is_err());
        assert!(!path.exists());
        let settings = StartupSettings {
            ssh_config: Some(dir.path().to_str().unwrap().into()),
            ..Default::default()
        };
        assert!(save_settings_checking_config(&path, &settings).is_err());
        assert!(!path.exists());
        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;
            let config = dir.path().join("config");
            fs::write(&config, "Host *\n").unwrap();
            let link = dir.path().join("link");
            symlink(&config, &link).unwrap();
            let settings = StartupSettings {
                ssh_config: Some(link.to_str().unwrap().into()),
                ..Default::default()
            };
            save_settings_checking_config(&path, &settings).unwrap();
            assert_eq!(load_settings(&path).unwrap(), settings);
        }
    }

    #[test]
    fn corrupt_startup_settings_do_not_overwrite_saved_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("startup.json");
        fs::write(&path, b"{broken").unwrap();
        assert!(load_settings(&path).is_err());
        assert_eq!(fs::read(path).unwrap(), b"{broken");
    }

    #[test]
    fn saved_config_refuses_invalid_contents_without_replacing_settings() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("startup.json");
        fs::write(&path, "sentinel").unwrap();
        let config = dir.path().join("config");
        for bytes in [
            vec![0xff],
            b"Include other\n".to_vec(),
            b"Match all\n".to_vec(),
            vec![b'x'; 1024 * 1024 + 1],
        ] {
            fs::write(&config, bytes).unwrap();
            let settings = StartupSettings {
                ssh_config: Some(config.to_str().unwrap().into()),
                ..Default::default()
            };
            assert!(save_settings_checking_config(&path, &settings).is_err());
            assert_eq!(fs::read(&path).unwrap(), b"sentinel");
        }
    }

    /// A non-UTF-8 path is refused lexically; the file is never created
    /// (APFS rejects such names before any check could run).
    #[cfg(unix)]
    #[test]
    fn saved_config_path_rejects_non_utf8_without_creating_it() {
        use std::os::unix::ffi::OsStrExt;
        let bad = Path::new(std::ffi::OsStr::from_bytes(b"/tmp/inspirum-\xff-config"));
        assert!(validate_ssh_config_path(bad).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn saved_config_follows_symlink_to_regular_file_and_refuses_fifo_without_blocking() {
        use std::os::unix::fs::symlink;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("startup.json");
        let config = dir.path().join("config");
        fs::write(&config, "Host *\n").unwrap();
        let link = dir.path().join("link");
        symlink(&config, &link).unwrap();
        let to_regular = StartupSettings {
            ssh_config: Some(link.to_str().unwrap().into()),
            ..Default::default()
        };
        save_settings_checking_config(&path, &to_regular).unwrap();
        assert_eq!(load_settings(&path).unwrap(), to_regular);

        let fifo = dir.path().join("fifo");
        assert!(
            std::process::Command::new("mkfifo")
                .arg(&fifo)
                .status()
                .unwrap()
                .success()
        );
        let fifo_link = dir.path().join("fifo-link");
        symlink(&fifo, &fifo_link).unwrap();
        for target in [&fifo, &fifo_link] {
            let settings = StartupSettings {
                ssh_config: Some(target.to_str().unwrap().into()),
                ..Default::default()
            };
            assert!(save_settings_checking_config(&path, &settings).is_err());
            assert_eq!(load_settings(&path).unwrap(), to_regular);
        }
    }

    /// Restart regression: a saved path whose file was removed is still a
    /// valid record. Behavior and path survive, behavior writes keep the
    /// path, Clear works and a checked replacement can be saved.
    #[test]
    fn missing_saved_ssh_config_keeps_record_and_allows_clear_and_replacement() {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("config");
        fs::write(&config, "Host *\n").unwrap();
        let path = dir.path().join("startup.json");
        let saved = StartupSettings {
            behavior: StartupBehavior::Profile {
                profile: "prod".into(),
            },
            ssh_config: Some(config.to_str().unwrap().into()),
        };
        save_settings_checking_config(&path, &saved).unwrap();
        fs::remove_file(&config).unwrap();
        assert_eq!(load_settings(&path).unwrap(), saved);

        let behavior_only = StartupSettings {
            behavior: StartupBehavior::None,
            ..saved.clone()
        };
        save_settings(&path, &behavior_only).unwrap();
        assert_eq!(load_settings(&path).unwrap(), behavior_only);

        let missing_replacement = StartupSettings {
            ssh_config: Some(dir.path().join("also-missing").to_str().unwrap().into()),
            ..saved.clone()
        };
        assert!(save_settings_checking_config(&path, &missing_replacement).is_err());
        assert_eq!(load_settings(&path).unwrap(), behavior_only);

        let cleared = StartupSettings {
            ssh_config: None,
            ..saved.clone()
        };
        save_settings(&path, &cleared).unwrap();
        assert_eq!(load_settings(&path).unwrap(), cleared);
        assert!(!fs::read_to_string(&path).unwrap().contains("ssh_config"));

        let replacement = dir.path().join("replacement");
        fs::write(&replacement, "Host *\n").unwrap();
        let replaced = StartupSettings {
            ssh_config: Some(replacement.to_str().unwrap().into()),
            ..saved
        };
        save_settings_checking_config(&path, &replaced).unwrap();
        assert_eq!(load_settings(&path).unwrap(), replaced);
    }

    #[test]
    fn load_rejects_lexically_invalid_saved_path_without_touching_disk() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("startup.json");
        for record in [
            r#"{"ssh_config":"relative/config"}"#,
            r#"{"ssh_config":"   "}"#,
            r#"{"ssh_config":"/tmp/bad\u0000path"}"#,
            r#"{"ssh_config":"/tmp/x","extra":1}"#,
        ] {
            fs::write(&path, record).unwrap();
            assert!(load_settings(&path).is_err(), "{record}");
            assert_eq!(fs::read_to_string(&path).unwrap(), record);
        }
    }

    #[test]
    fn saved_config_path_rejects_controls_and_blank_paths() {
        let dir = tempfile::tempdir().unwrap();
        for value in [
            "",
            "  ",
            "/tmp/bad\npath",
            "/tmp/bad\0path",
            &format!("/{}", "x".repeat(4096)),
        ] {
            let settings = StartupSettings {
                ssh_config: Some(value.into()),
                ..Default::default()
            };
            assert!(save_settings(&dir.path().join("startup.json"), &settings).is_err());
        }
    }

    #[test]
    fn saved_config_keeps_legal_surrounding_spaces_in_path() {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join(" config ");
        fs::write(&config, "Host *\n").unwrap();
        let path = dir.path().join("startup.json");
        let settings = StartupSettings {
            ssh_config: Some(config.to_str().unwrap().into()),
            ..Default::default()
        };
        save_settings(&path, &settings).unwrap();
        assert_eq!(
            load_settings(&path).unwrap().ssh_config,
            settings.ssh_config
        );
    }

    #[test]
    fn workspace_startup_is_only_a_path_choice() {
        let settings = StartupSettings {
            behavior: StartupBehavior::Workspace {
                path: "/tmp/layout.json".into(),
            },
            ssh_config: None,
        };
        settings.validate().unwrap();
    }
}
