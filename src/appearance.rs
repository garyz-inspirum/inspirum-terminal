//! Persistent global terminal presentation and pointer interaction preferences.
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
};

const MAX_SETTINGS_BYTES: usize = 64 * 1024;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FontFamilyChoice {
    #[default]
    Monospace,
    Proportional,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThemePreset {
    #[default]
    ClassicDark,
    Light,
    SolarizedDark,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CursorStyle {
    #[default]
    Block,
    Underline,
    Beam,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MousePasteAction {
    #[default]
    Disabled,
    PasteClipboard,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AppearanceSettings {
    pub font_family: FontFamilyChoice,
    pub font_size: f32,
    pub theme: ThemePreset,
    /// Optional custom foreground override in #RRGGBB form.
    pub foreground: Option<String>,
    /// Optional custom background override in #RRGGBB form.
    pub background: Option<String>,
    pub cursor_style: CursorStyle,
    pub select_to_copy: bool,
    pub middle_click: MousePasteAction,
    pub right_click: MousePasteAction,
    pub hide_pointer_while_typing: bool,
}

impl Default for AppearanceSettings {
    fn default() -> Self {
        Self {
            font_family: FontFamilyChoice::Monospace,
            font_size: 14.0,
            theme: ThemePreset::ClassicDark,
            foreground: None,
            background: None,
            cursor_style: CursorStyle::Block,
            select_to_copy: false,
            middle_click: MousePasteAction::Disabled,
            right_click: MousePasteAction::Disabled,
            hide_pointer_while_typing: false,
        }
    }
}

impl AppearanceSettings {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.font_size.is_finite() && (8.0..=48.0).contains(&self.font_size),
            "terminal font size must be between 8 and 48 points"
        );
        if let Some(value) = &self.foreground {
            validate_hex(value, "foreground")?;
        }
        if let Some(value) = &self.background {
            validate_hex(value, "background")?;
        }
        Ok(())
    }

    pub fn contrast_warning(&self) -> Option<String> {
        let foreground = self
            .foreground
            .as_deref()
            .or_else(|| preset_colors(self.theme).map(|pair| pair.0))?;
        let background = self
            .background
            .as_deref()
            .or_else(|| preset_colors(self.theme).map(|pair| pair.1))?;
        let ratio = contrast_ratio(parse_hex(foreground)?, parse_hex(background)?);
        (ratio < 3.0).then(|| {
            format!(
                "Low terminal text contrast ({ratio:.1}:1). Choose a more distinct foreground/background combination."
            )
        })
    }
}

pub fn settings_path(profiles_path: &Path) -> PathBuf {
    let name = profiles_path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("sessions.json");
    profiles_path.with_file_name(format!("{name}.appearance.json"))
}

pub fn load_settings(path: &Path) -> Result<AppearanceSettings> {
    let file = match fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(AppearanceSettings::default());
        }
        Err(error) => return Err(error).context("open appearance settings"),
    };
    let mut bytes = Vec::new();
    file.take((MAX_SETTINGS_BYTES + 1) as u64)
        .read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() <= MAX_SETTINGS_BYTES,
        "appearance settings exceed 64 KiB"
    );
    let settings: AppearanceSettings =
        serde_json::from_slice(&bytes).context("parse appearance settings")?;
    settings.validate()?;
    Ok(settings)
}

pub fn save_settings(path: &Path, settings: &AppearanceSettings) -> Result<()> {
    settings.validate()?;
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    fs::create_dir_all(parent).context("create appearance settings directory")?;
    let mut bytes = serde_json::to_vec_pretty(settings)?;
    bytes.push(b'\n');
    ensure!(
        bytes.len() <= MAX_SETTINGS_BYTES,
        "appearance settings exceed 64 KiB"
    );
    let mut temp =
        tempfile::NamedTempFile::new_in(parent).context("create appearance settings temp file")?;
    temp.write_all(&bytes)?;
    temp.as_file().sync_all()?;
    temp.persist(path)
        .map_err(|error| error.error)
        .context("replace appearance settings")?;
    Ok(())
}

pub fn preset_palette(theme: ThemePreset) -> [&'static str; 18] {
    match theme {
        ThemePreset::ClassicDark => [
            "#d8d8d8", "#181818", "#181818", "#ac4242", "#90a959", "#f4bf75", "#6a9fb5", "#aa759f",
            "#75b5aa", "#d8d8d8", "#6b6b6b", "#c55555", "#aac474", "#feca88", "#82b8c8", "#c28cb8",
            "#93d3c3", "#f8f8f8",
        ],
        ThemePreset::Light => [
            "#202020", "#f7f7f7", "#202020", "#b03030", "#427a2e", "#8a6500", "#315da8", "#8b3f8f",
            "#267a78", "#dddddd", "#686868", "#d05050", "#5c963f", "#aa7c00", "#4f7ecb", "#aa5aae",
            "#359996", "#ffffff",
        ],
        ThemePreset::SolarizedDark => [
            "#839496", "#002b36", "#073642", "#dc322f", "#859900", "#b58900", "#268bd2", "#d33682",
            "#2aa198", "#eee8d5", "#002b36", "#cb4b16", "#586e75", "#657b83", "#839496", "#6c71c4",
            "#93a1a1", "#fdf6e3",
        ],
    }
}

fn preset_colors(theme: ThemePreset) -> Option<(&'static str, &'static str)> {
    let palette = preset_palette(theme);
    Some((palette[0], palette[1]))
}

fn validate_hex(value: &str, label: &str) -> Result<()> {
    ensure!(
        parse_hex(value).is_some(),
        "{label} color must use #RRGGBB format"
    );
    Ok(())
}

fn parse_hex(value: &str) -> Option<[u8; 3]> {
    if value.len() != 7 || !value.starts_with('#') {
        return None;
    }
    Some([
        u8::from_str_radix(&value[1..3], 16).ok()?,
        u8::from_str_radix(&value[3..5], 16).ok()?,
        u8::from_str_radix(&value[5..7], 16).ok()?,
    ])
}

fn linear(component: u8) -> f32 {
    let value = f32::from(component) / 255.0;
    if value <= 0.03928 {
        value / 12.92
    } else {
        ((value + 0.055) / 1.055).powf(2.4)
    }
}

fn luminance(rgb: [u8; 3]) -> f32 {
    0.2126 * linear(rgb[0]) + 0.7152 * linear(rgb[1]) + 0.0722 * linear(rgb[2])
}

fn contrast_ratio(left: [u8; 3], right: [u8; 3]) -> f32 {
    let a = luminance(left);
    let b = luminance(right);
    let (bright, dark) = if a >= b { (a, b) } else { (b, a) };
    (bright + 0.05) / (dark + 0.05)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_settings_keep_existing_default_terminal_appearance() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            load_settings(&dir.path().join("missing.json")).unwrap(),
            AppearanceSettings::default()
        );
    }

    #[test]
    fn settings_round_trip_and_reject_unsafe_values() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("appearance.json");
        let settings = AppearanceSettings {
            font_size: 18.0,
            theme: ThemePreset::SolarizedDark,
            cursor_style: CursorStyle::Beam,
            select_to_copy: true,
            middle_click: MousePasteAction::PasteClipboard,
            hide_pointer_while_typing: true,
            ..AppearanceSettings::default()
        };
        save_settings(&path, &settings).unwrap();
        assert_eq!(load_settings(&path).unwrap(), settings);

        let mut invalid = settings;
        invalid.font_size = 100.0;
        assert!(invalid.validate().is_err());
        invalid.font_size = 14.0;
        invalid.foreground = Some("red".into());
        assert!(invalid.validate().is_err());
    }

    #[test]
    fn unreadable_custom_theme_warns() {
        let settings = AppearanceSettings {
            foreground: Some("#101010".into()),
            background: Some("#111111".into()),
            ..AppearanceSettings::default()
        };
        assert!(settings.contrast_warning().is_some());
    }
}
