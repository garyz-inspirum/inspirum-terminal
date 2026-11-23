//! WindTerm-style chrome theme (spec section 2-3). The terminal palette stays
//! independent (`TerminalAppearance`); this only covers the application chrome.

use std::sync::atomic::{AtomicU8, Ordering};

use iced::{Color, Theme};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(super) enum ChromeTheme {
    #[default]
    Light,
    Dark,
}

impl ChromeTheme {
    #[allow(dead_code)] // consumed by the View > Theme menu in a later stage
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::Light => "light",
            Self::Dark => "dark",
        }
    }

    pub(super) fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "light" => Some(Self::Light),
            "dark" => Some(Self::Dark),
            _ => None,
        }
    }
}

#[allow(dead_code)] // several tokens are consumed by later UI stages
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Palette {
    pub chrome: Color,
    pub dock: Color,
    pub list: Color,
    pub header: Color,
    pub tab_active: Color,
    pub tab_inactive: Color,
    pub raised: Color,
    pub hover: Color,
    pub selected_row: Color,
    pub border: Color,
    pub text: Color,
    pub text_dock: Color,
    pub muted: Color,
    /// High-visibility outline for selected dialog choices.
    pub selection_indicator: Color,
    /// Contextual preflight text; unlike the orange marker, readable on chrome.
    pub warning_text: Color,
    pub accent: Color,
    pub magenta: Color,
    pub success: Color,
    pub danger: Color,
    pub status_bg: Color,
    pub status_text: Color,
}

impl Palette {
    /// Primary field captions and entered values, distinct from supporting copy.
    pub fn field_label(self) -> Color {
        self.text
    }

    pub fn field_help(self) -> Color {
        self.muted
    }

    pub fn input_value(self) -> Color {
        self.field_label()
    }

    pub fn input_placeholder(self) -> Color {
        self.field_help()
    }
}

const fn hex(value: u32) -> Color {
    Color::from_rgb8(
        ((value >> 16) & 0xff) as u8,
        ((value >> 8) & 0xff) as u8,
        (value & 0xff) as u8,
    )
}

pub(super) const LIGHT: Palette = Palette {
    chrome: hex(0xECECEC),
    dock: hex(0xF0F0F0),
    list: hex(0xFFFFFF),
    header: hex(0xF2F2F2),
    tab_active: hex(0xF5F5F5),
    tab_inactive: hex(0xDDDDDD),
    raised: hex(0xFFFFFF),
    hover: hex(0xE4E4E4),
    selected_row: hex(0xD8D8D8),
    border: hex(0xBFBFBF),
    text: hex(0x151515),
    text_dock: hex(0x333333),
    muted: hex(0x5C5C5C),
    selection_indicator: hex(0x0066B3),
    warning_text: hex(0x895000),
    accent: hex(0x1E90FF),
    magenta: hex(0xFF1493),
    success: hex(0x339333),
    danger: hex(0xD32F2F),
    status_bg: hex(0x007ACC),
    status_text: hex(0xFFFFFF),
};

pub(super) const DARK: Palette = Palette {
    chrome: hex(0x1E1E1E),
    dock: hex(0x191919),
    list: hex(0x1E1E1E),
    header: hex(0x222222),
    tab_active: hex(0x1E1E1E),
    tab_inactive: hex(0x333333),
    raised: hex(0x333333),
    hover: hex(0x2A2A2A),
    selected_row: hex(0x3A3A3A),
    border: hex(0x444444),
    text: hex(0xBFBFBF),
    text_dock: hex(0xBFBFBF),
    muted: hex(0xAAAAAA),
    selection_indicator: hex(0x70B8FF),
    warning_text: hex(0xF6B84B),
    accent: hex(0x2196F3),
    magenta: hex(0xFF0099),
    success: hex(0xA6E22E),
    danger: hex(0xFF5C5C),
    status_bg: hex(0x007ACC),
    status_text: hex(0xFFFFFF),
};

pub(super) const WARNING: Color = hex(0xF6A623);

// Panel marker colours (both themes).
#[allow(dead_code)] // consumed by the dock stage
pub(super) mod marker {
    use super::hex;
    use iced::Color;
    pub const SENDER: Color = hex(0xA16BE8);
    pub const SESSION: Color = hex(0x009FFF);
    pub const OUTLINE: Color = hex(0x91DDF2);
    pub const TRANSFER: Color = hex(0x00CCCC);
    pub const FILER: Color = hex(0x555BA9);
    pub const EXPLORER: Color = hex(0x7CB342);
    pub const SHELL: Color = hex(0xFF0099);
    pub const TUNNELS: Color = hex(0xF6A623);
}

// Metrics (spec section 3), in logical px before the global scale.
#[allow(dead_code)] // consumed by later layout stages
pub(super) mod metrics {
    pub const UI_TEXT: f32 = 13.0;
    pub const STATUS_TEXT: f32 = 12.0;
    pub const ROW_HEIGHT: f32 = 23.0;
    pub const TABLE_HEADER_HEIGHT: f32 = 20.0;
    pub const STRIP_HEIGHT: f32 = 26.0;
    pub const MENU_BAR_HEIGHT: f32 = 32.0;
    pub const STATUS_BAR_HEIGHT: f32 = 22.0;
}

static CURRENT: AtomicU8 = AtomicU8::new(0);

pub(super) fn current() -> ChromeTheme {
    match CURRENT.load(Ordering::Relaxed) {
        1 => ChromeTheme::Dark,
        _ => ChromeTheme::Light,
    }
}

pub(super) fn set(theme: ChromeTheme) {
    let value = match theme {
        ChromeTheme::Light => 0,
        ChromeTheme::Dark => 1,
    };
    CURRENT.store(value, Ordering::Relaxed);
}

pub(super) fn palette() -> &'static Palette {
    match current() {
        ChromeTheme::Light => &LIGHT,
        ChromeTheme::Dark => &DARK,
    }
}

pub(super) fn init_from_env() {
    if let Some(theme) = std::env::var("INSPIRUM_THEME")
        .ok()
        .as_deref()
        .and_then(ChromeTheme::parse)
    {
        set(theme);
    }
}

pub(super) fn iced_theme() -> Theme {
    let p = palette();
    let name = match current() {
        ChromeTheme::Light => "WindTerm Light",
        ChromeTheme::Dark => "WindTerm Dark",
    };
    Theme::custom(
        name,
        iced::theme::Palette {
            background: p.list,
            text: p.text,
            primary: p.accent,
            success: p.success,
            warning: WARNING,
            danger: p.danger,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn light_and_dark_palettes_differ() {
        assert_ne!(LIGHT, DARK);
        assert_ne!(LIGHT.text, DARK.text);
        assert_ne!(LIGHT.list, DARK.list);
    }

    #[test]
    fn set_and_current_round_trip() {
        let before = current();
        set(ChromeTheme::Dark);
        assert_eq!(current(), ChromeTheme::Dark);
        assert_eq!(palette(), &DARK);
        set(ChromeTheme::Light);
        assert_eq!(current(), ChromeTheme::Light);
        assert_eq!(palette(), &LIGHT);
        set(before);
    }

    #[test]
    fn parse_accepts_known_names() {
        assert_eq!(ChromeTheme::parse("dark"), Some(ChromeTheme::Dark));
        assert_eq!(ChromeTheme::parse(" Light "), Some(ChromeTheme::Light));
        assert_eq!(ChromeTheme::parse("blue"), None);
        for theme in [ChromeTheme::Light, ChromeTheme::Dark] {
            assert_eq!(ChromeTheme::parse(theme.as_str()), Some(theme));
        }
        assert_eq!(ChromeTheme::default(), ChromeTheme::Light);
    }

    /// WCAG 2.1 relative luminance for opaque sRGB chrome tokens.
    fn luminance(color: Color) -> f32 {
        let linear = |channel: f32| {
            if channel <= 0.04045 {
                channel / 12.92
            } else {
                ((channel + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * linear(color.r) + 0.7152 * linear(color.g) + 0.0722 * linear(color.b)
    }

    fn contrast(a: Color, b: Color) -> f32 {
        let (low, high) = (luminance(a), luminance(b));
        (high.max(low) + 0.05) / (high.min(low) + 0.05)
    }

    #[test]
    fn field_hierarchy_is_readable_and_distinct_in_both_themes() {
        for p in [LIGHT, DARK] {
            assert_eq!(p.field_label(), p.text);
            assert_eq!(p.field_help(), p.muted);
            assert_ne!(p.field_label(), p.field_help());
            for surface in [p.chrome, p.dock, p.list, p.raised, p.hover, p.selected_row] {
                assert!(
                    contrast(p.field_label(), surface) >= 4.5,
                    "label on {surface:?}"
                );
                assert!(
                    contrast(p.field_help(), surface) >= 4.5,
                    "help on {surface:?}"
                );
            }
            assert_eq!(p.input_value(), p.field_label());
            assert_eq!(p.input_placeholder(), p.field_help());
            assert_ne!(p.input_value(), p.input_placeholder());
        }
        assert_eq!(LIGHT.field_label(), hex(0x151515));
        assert_eq!(LIGHT.field_help(), hex(0x5C5C5C));
    }

    #[test]
    fn input_outlines_meet_non_text_contrast_on_neighbouring_fills() {
        // WCAG 1.4.11: a text field's boundary must reach 3:1 against both
        // its own fill and every surface it sits on, in both themes.
        for p in [LIGHT, DARK] {
            for surface in [p.chrome, p.dock, p.list, p.raised, p.hover, p.selected_row] {
                assert!(
                    contrast(p.input_border(), surface) >= 3.0,
                    "input outline on {surface:?}"
                );
                assert!(
                    contrast(p.input_focus_border(), surface) >= 3.0,
                    "focused input outline on {surface:?}"
                );
            }
            assert_ne!(p.input_border(), p.input_focus_border());
        }
        // Regression: the shared divider token is too faint for inputs.
        assert!(contrast(LIGHT.border, LIGHT.dock) < 3.0);
    }

    #[test]
    fn small_chrome_text_meets_wcag_aa_on_all_relevant_surfaces() {
        // Direct constants avoid changing the process-global runtime theme in parallel tests.
        for p in [LIGHT, DARK] {
            // A selected row must remain visibly distinct from its list background.
            for (list, selected) in [
                (p.list.r, p.selected_row.r),
                (p.list.g, p.selected_row.g),
                (p.list.b, p.selected_row.b),
            ] {
                assert!(
                    (list - selected).abs() * 255.0 >= 25.0,
                    "selected row visibility"
                );
            }
            for surface in [p.chrome, p.dock, p.list, p.raised, p.hover, p.selected_row] {
                assert!(contrast(p.muted, surface) >= 4.5, "muted on {surface:?}");
            }
            assert!(contrast(p.muted, p.list) >= 4.5, "input placeholder");
            assert!(contrast(p.text, p.list) >= 4.5, "input value");
            assert!(contrast(p.text, p.selected_row) >= 4.5, "selected choice");
            assert!(contrast(p.warning_text, p.dock) >= 4.5, "preflight warning");
            assert!(contrast(Color::BLACK, p.accent) >= 4.5, "primary action");
            assert!(contrast(p.selection_indicator, p.selected_row) >= 3.0);
        }
    }
}
