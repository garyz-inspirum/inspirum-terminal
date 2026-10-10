//! Persisted terminal appearance and pointer interaction preferences.
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs,
    io::Read,
    path::{Path, PathBuf},
};

const MAX_APPEARANCE_BYTES: usize = 64 * 1024;

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TerminalPalette {
    #[default]
    DefaultDark,
    Light,
    HighContrast,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TerminalCursorStyle {
    #[default]
    Block,
    Underline,
    Beam,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TerminalFontFamily {
    #[default]
    Monospace,
    Proportional,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct TerminalAppearance {
    pub font_family: TerminalFontFamily,
    pub font_size: f32,
    pub palette: TerminalPalette,
    pub foreground: Option<String>,
    pub background: Option<String>,
    pub cursor_style: TerminalCursorStyle,
    pub opacity: f32,
    pub select_to_copy: bool,
    pub middle_click_paste: bool,
    pub right_click_paste: bool,
    pub hide_pointer_while_typing: bool,
}

impl Default for TerminalAppearance {
    fn default() -> Self {
        Self {
            font_family: TerminalFontFamily::Monospace,
            font_size: 14.0,
            palette: TerminalPalette::DefaultDark,
            foreground: None,
            background: None,
            cursor_style: TerminalCursorStyle::Block,
            opacity: 1.0,
            select_to_copy: false,
            middle_click_paste: false,
            right_click_paste: false,
            hide_pointer_while_typing: false,
        }
    }
}

impl TerminalAppearance {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.font_size.is_finite() && (8.0..=36.0).contains(&self.font_size),
            "terminal font size must be from 8 to 36"
        );
        ensure!(
            self.opacity.is_finite() && (0.35..=1.0).contains(&self.opacity),
            "terminal opacity must be from 0.35 to 1.0"
        );
        for (label, value) in [
            ("foreground", self.foreground.as_deref()),
            ("background", self.background.as_deref()),
        ] {
            if let Some(value) = value {
                ensure!(is_hex_color(value), "{label} must be #RRGGBB");
            }
        }
        Ok(())
    }

    pub fn contrast_warning(&self) -> Option<String> {
        let foreground = self
            .foreground
            .as_deref()
            .and_then(parse_hex_rgb)
            .or_else(|| parse_hex_rgb(self.palette_defaults().0))?;
        let background = self
            .background
            .as_deref()
            .and_then(parse_hex_rgb)
            .or_else(|| parse_hex_rgb(self.palette_defaults().1))?;
        let ratio = contrast_ratio(foreground, background);
        (ratio < 3.0).then(|| format!(
            "Foreground/background contrast is low ({ratio:.1}:1); 3:1 or higher is recommended."
        ))
    }

    pub fn palette_defaults(&self) -> (&'static str, &'static str) {
        match self.palette {
            TerminalPalette::DefaultDark => ("#d8d8d8", "#181818"),
            TerminalPalette::Light => ("#202124", "#f7f7f7"),
            TerminalPalette::HighContrast => ("#ffffff", "#000000"),
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct AppearanceOverride {
    pub font_family: Option<TerminalFontFamily>,
    pub font_size: Option<f32>,
    pub palette: Option<TerminalPalette>,
    pub foreground: Option<String>,
    pub background: Option<String>,
    pub cursor_style: Option<TerminalCursorStyle>,
    pub select_to_copy: Option<bool>,
    pub middle_click_paste: Option<bool>,
    pub right_click_paste: Option<bool>,
    pub hide_pointer_while_typing: Option<bool>,
}

impl AppearanceOverride {
    pub fn apply_to(&self, base: &TerminalAppearance) -> TerminalAppearance {
        TerminalAppearance {
            font_family: self.font_family.unwrap_or(base.font_family),
            font_size: self.font_size.unwrap_or(base.font_size),
            palette: self.palette.unwrap_or(base.palette),
            foreground: self.foreground.clone().or_else(|| base.foreground.clone()),
            background: self.background.clone().or_else(|| base.background.clone()),
            cursor_style: self.cursor_style.unwrap_or(base.cursor_style),
            opacity: base.opacity,
            select_to_copy: self.select_to_copy.unwrap_or(base.select_to_copy),
            middle_click_paste: self.middle_click_paste.unwrap_or(base.middle_click_paste),
            right_click_paste: self.right_click_paste.unwrap_or(base.right_click_paste),
            hide_pointer_while_typing: self
                .hide_pointer_while_typing
                .unwrap_or(base.hide_pointer_while_typing),
        }
    }

    pub fn validate(&self) -> Result<()> {
        if let Some(size) = self.font_size {
            ensure!(
                size.is_finite() && (8.0..=36.0).contains(&size),
                "profile terminal font size must be from 8 to 36"
            );
        }
        for (label, value) in [
            ("profile foreground", self.foreground.as_deref()),
            ("profile background", self.background.as_deref()),
        ] {
            if let Some(value) = value {
                ensure!(is_hex_color(value), "{label} must be #RRGGBB");
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct AppearanceSettings {
    pub version: u32,
    pub global: TerminalAppearance,
    pub profiles: BTreeMap<String, AppearanceOverride>,
}

impl Default for AppearanceSettings {
    fn default() -> Self {
        Self {
            version: 1,
            global: TerminalAppearance::default(),
            profiles: BTreeMap::new(),
        }
    }
}

impl AppearanceSettings {
    pub fn validate(&self) -> Result<()> {
        ensure!(self.version == 1, "unsupported appearance settings version");
        self.global.validate()?;
        ensure!(
            self.profiles.len() <= 1000,
            "at most 1000 profile appearance overrides are supported"
        );
        for (name, override_) in &self.profiles {
            ensure!(
                !name.trim().is_empty()
                    && name.len() <= 1024
                    && !name.chars().any(char::is_control),
                "profile appearance key is invalid"
            );
            override_.validate()?;
        }
        Ok(())
    }

    pub fn effective_for(&self, profile_name: &str) -> TerminalAppearance {
        self.profiles
            .get(profile_name)
            .map(|override_| override_.apply_to(&self.global))
            .unwrap_or_else(|| self.global.clone())
    }
}

pub fn settings_path(profiles_path: &Path) -> PathBuf {
    let file_name = profiles_path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("sessions.json");
    profiles_path.with_file_name(format!("{file_name}.appearance.json"))
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
    file.take((MAX_APPEARANCE_BYTES + 1) as u64)
        .read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() <= MAX_APPEARANCE_BYTES,
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
        bytes.len() <= MAX_APPEARANCE_BYTES,
        "appearance settings exceed 64 KiB"
    );
    let mut temp =
        tempfile::NamedTempFile::new_in(parent).context("create appearance settings temp file")?;
    use std::io::Write;
    temp.write_all(&bytes)?;
    temp.as_file().sync_all()?;
    temp.persist(path)
        .map_err(|error| error.error)
        .context("replace appearance settings")?;
    Ok(())
}

fn is_hex_color(value: &str) -> bool {
    value.len() == 7
        && value.starts_with('#')
        && value[1..].bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn parse_hex_rgb(value: &str) -> Option<(u8, u8, u8)> {
    if !is_hex_color(value) {
        return None;
    }
    Some((
        u8::from_str_radix(&value[1..3], 16).ok()?,
        u8::from_str_radix(&value[3..5], 16).ok()?,
        u8::from_str_radix(&value[5..7], 16).ok()?,
    ))
}

fn channel_luminance(value: u8) -> f32 {
    let value = value as f32 / 255.0;
    if value <= 0.04045 {
        value / 12.92
    } else {
        ((value + 0.055) / 1.055).powf(2.4)
    }
}

fn contrast_ratio(foreground: (u8, u8, u8), background: (u8, u8, u8)) -> f32 {
    let luminance = |(r, g, b): (u8, u8, u8)| {
        0.2126 * channel_luminance(r)
            + 0.7152 * channel_luminance(g)
            + 0.0722 * channel_luminance(b)
    };
    let a = luminance(foreground);
    let b = luminance(background);
    let high = a.max(b);
    let low = a.min(b);
    (high + 0.05) / (low + 0.05)
}

pub(crate) fn terminal_theme(appearance: &TerminalAppearance) -> terminal_core::TerminalTheme {
    let mut palette = terminal_core::ColorPalette::default();
    match appearance.palette {
        TerminalPalette::DefaultDark => {}
        TerminalPalette::Light => {
            palette.foreground = "#202124".into();
            palette.background = "#f7f7f7".into();
            palette.black = "#202124".into();
            palette.red = "#b3261e".into();
            palette.green = "#2e7d32".into();
            palette.yellow = "#8a6d00".into();
            palette.blue = "#1565c0".into();
            palette.magenta = "#8e24aa".into();
            palette.cyan = "#00796b".into();
            palette.white = "#eceff1".into();
            palette.bright_black = "#5f6368".into();
            palette.bright_red = "#d93025".into();
            palette.bright_green = "#188038".into();
            palette.bright_yellow = "#a86f00".into();
            palette.bright_blue = "#1a73e8".into();
            palette.bright_magenta = "#a142f4".into();
            palette.bright_cyan = "#00897b".into();
            palette.bright_white = "#ffffff".into();
            palette.dim_foreground = "#5f6368".into();
        }
        TerminalPalette::HighContrast => {
            palette.foreground = "#ffffff".into();
            palette.background = "#000000".into();
            palette.black = "#000000".into();
            palette.red = "#ff5555".into();
            palette.green = "#55ff55".into();
            palette.yellow = "#ffff55".into();
            palette.blue = "#5555ff".into();
            palette.magenta = "#ff55ff".into();
            palette.cyan = "#55ffff".into();
            palette.white = "#ffffff".into();
            palette.bright_black = "#808080".into();
            palette.bright_red = "#ff8080".into();
            palette.bright_green = "#80ff80".into();
            palette.bright_yellow = "#ffff80".into();
            palette.bright_blue = "#8080ff".into();
            palette.bright_magenta = "#ff80ff".into();
            palette.bright_cyan = "#80ffff".into();
            palette.bright_white = "#ffffff".into();
            palette.dim_foreground = "#b0b0b0".into();
        }
    }
    if let Some(foreground) = &appearance.foreground {
        palette.foreground = foreground.clone();
    }
    if let Some(background) = &appearance.background {
        palette.background = background.clone();
    }
    terminal_core::TerminalTheme::new(Box::new(palette))
}

pub(crate) fn full_profile_override(appearance: &TerminalAppearance) -> AppearanceOverride {
    AppearanceOverride {
        font_family: Some(appearance.font_family),
        font_size: Some(appearance.font_size),
        palette: Some(appearance.palette),
        foreground: appearance.foreground.clone(),
        background: appearance.background.clone(),
        cursor_style: Some(appearance.cursor_style),
        select_to_copy: Some(appearance.select_to_copy),
        middle_click_paste: Some(appearance.middle_click_paste),
        right_click_paste: Some(appearance.right_click_paste),
        hide_pointer_while_typing: Some(appearance.hide_pointer_while_typing),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_settings_preserve_existing_default_terminal_appearance() {
        let dir = tempfile::tempdir().unwrap();
        let settings = load_settings(&dir.path().join("missing.json")).unwrap();
        assert_eq!(settings, AppearanceSettings::default());
        assert_eq!(settings.global.font_size, 14.0);
        assert_eq!(settings.global.palette, TerminalPalette::DefaultDark);
        assert!(!settings.global.select_to_copy);
        assert!(!settings.global.middle_click_paste);
        assert!(!settings.global.right_click_paste);
    }

    #[test]
    fn round_trip_and_profile_override_are_deterministic() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("appearance.json");
        let mut settings = AppearanceSettings::default();
        settings.global.font_size = 16.0;
        settings.profiles.insert(
            "prod".into(),
            AppearanceOverride {
                palette: Some(TerminalPalette::HighContrast),
                cursor_style: Some(TerminalCursorStyle::Beam),
                ..AppearanceOverride::default()
            },
        );
        save_settings(&path, &settings).unwrap();
        assert_eq!(load_settings(&path).unwrap(), settings);
        let prod = settings.effective_for("prod");
        assert_eq!(prod.font_size, 16.0);
        assert_eq!(prod.palette, TerminalPalette::HighContrast);
        assert_eq!(prod.cursor_style, TerminalCursorStyle::Beam);
    }

    #[test]
    fn invalid_and_low_contrast_colors_are_handled_safely() {
        let mut appearance = TerminalAppearance {
            foreground: Some("not-a-color".into()),
            ..TerminalAppearance::default()
        };
        assert!(appearance.validate().is_err());

        appearance.foreground = Some("#202020".into());
        appearance.background = Some("#181818".into());
        appearance.validate().unwrap();
        assert!(appearance.contrast_warning().is_some());
    }

    #[test]
    fn unknown_fields_are_rejected_but_missing_fields_migrate_to_defaults() {
        let migrated: AppearanceSettings =
            serde_json::from_str(r#"{"version":1,"global":{"font_size":15.0}}"#).unwrap();
        assert_eq!(migrated.global.font_size, 15.0);
        assert_eq!(migrated.global.cursor_style, TerminalCursorStyle::Block);

        let unknown = serde_json::from_str::<AppearanceSettings>(
            r#"{"version":1,"global":{"unexpected":true}}"#,
        );
        assert!(unknown.is_err());
    }
}
