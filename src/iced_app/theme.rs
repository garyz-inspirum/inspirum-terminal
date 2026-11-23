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
    /// Outline for interactive controls (text fields), >= 3:1 on all fills.
    pub control_outline: Color,
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

    /// Resting text-field outline. The shared `border` divider is too faint
    /// (1.61:1 on the light dock); controls need 3:1 against every fill.
    pub fn input_border(self) -> Color {
        self.control_outline
    }

    /// Focused text-field outline; `accent` misses 3:1 on the light dock.
    pub fn input_focus_border(self) -> Color {
        self.selection_indicator
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
    control_outline: hex(0x767676),
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
    control_outline: hex(0x8A8A8A),
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

/// Light-theme tint behind the focused split pane header: a flat pale blue
/// that keeps dark text >= 4.5:1 and the blue focus rule >= 3:1.
const LIGHT_FOCUS_HEADER: Color = hex(0xD9E8F5);

/// Flat chrome state cue: a fill plus an optional accent rule (top rule for
/// tabs/panes, underline for menu titles). `rule_height` is the reserved
/// strip in logical px; layout keeps it constant so terminal rows never move.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct StateCue {
    pub fill: Color,
    pub rule: Option<Color>,
    pub rule_height: f32,
}

pub(super) fn palette_for(theme: ChromeTheme) -> &'static Palette {
    match theme {
        ChromeTheme::Light => &LIGHT,
        ChromeTheme::Dark => &DARK,
    }
}

/// Menu-bar title. Light marks the open menu with a darker fill and a blue
/// underline (hover alone is only the pale `hover` fill); Dark is unchanged.
pub(super) fn menu_title_cue(theme: ChromeTheme, open: bool) -> StateCue {
    let p = palette_for(theme);
    let (fill, rule) = match (theme, open) {
        (_, false) => (p.chrome, None),
        (ChromeTheme::Light, true) => (p.selected_row, Some(p.selection_indicator)),
        (ChromeTheme::Dark, true) => (p.hover, None),
    };
    StateCue {
        fill,
        rule,
        rule_height: 2.0,
    }
}

/// Hovered drop-down entry fill.
pub(super) fn menu_item_hover_fill(theme: ChromeTheme) -> Color {
    let p = palette_for(theme);
    match theme {
        ChromeTheme::Light => p.selected_row,
        ChromeTheme::Dark => p.hover,
    }
}

/// Workspace session tab. Inactive tabs keep normal text (not disabled look).
pub(super) fn session_tab_cue(theme: ChromeTheme, active: bool) -> StateCue {
    let p = palette_for(theme);
    match (theme, active) {
        (_, false) => StateCue {
            fill: p.tab_inactive,
            rule: None,
            rule_height: 0.0,
        },
        (ChromeTheme::Light, true) => StateCue {
            fill: p.list,
            rule: Some(p.selection_indicator),
            rule_height: 3.0,
        },
        (ChromeTheme::Dark, true) => StateCue {
            fill: p.tab_active,
            rule: Some(p.accent),
            rule_height: 2.0,
        },
    }
}

/// Split-pane header. The rule strip is always 2px so focus changes never
/// alter the terminal grid height.
pub(super) fn pane_header_cue(theme: ChromeTheme, focused: bool) -> StateCue {
    let p = palette_for(theme);
    let (fill, rule) = match (theme, focused) {
        (_, false) => (p.header, None),
        (ChromeTheme::Light, true) => (LIGHT_FOCUS_HEADER, Some(p.selection_indicator)),
        (ChromeTheme::Dark, true) => (p.header, Some(p.accent)),
    };
    StateCue {
        fill,
        rule,
        rule_height: 2.0,
    }
}

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
    palette_for(current())
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

    /// Smallest per-channel difference, in 8-bit steps, between two fills.
    fn channel_delta(a: Color, b: Color) -> f32 {
        [(a.r - b.r), (a.g - b.g), (a.b - b.b)]
            .iter()
            .map(|d| d.abs() * 255.0)
            .fold(f32::MAX, f32::min)
    }

    #[test]
    fn light_open_menu_title_has_readable_fill_and_underline() {
        let open = menu_title_cue(ChromeTheme::Light, true);
        let closed = menu_title_cue(ChromeTheme::Light, false);
        assert_ne!(open.fill, LIGHT.hover, "open must not look like hover");
        assert!(channel_delta(open.fill, LIGHT.chrome) >= 15.0, "open fill");
        let rule = open.rule.expect("open title needs an underline");
        assert!(contrast(rule, open.fill) >= 3.0, "underline on open fill");
        assert!(contrast(rule, LIGHT.chrome) >= 3.0, "underline on menu bar");
        assert_eq!(closed.rule, None);
        for cue in [open, closed] {
            assert!(contrast(LIGHT.text, cue.fill) >= 4.5, "title text");
        }
    }

    #[test]
    fn light_menu_item_hover_is_visible_on_popup() {
        let fill = menu_item_hover_fill(ChromeTheme::Light);
        assert!(channel_delta(fill, LIGHT.list) >= 35.0, "hover vs popup");
        assert!(contrast(LIGHT.text, fill) >= 4.5);
        assert!(contrast(LIGHT.muted, fill) >= 4.5, "shortcut text");
    }

    #[test]
    fn light_active_session_tab_cue_exceeds_hairline() {
        let active = session_tab_cue(ChromeTheme::Light, true);
        let inactive = session_tab_cue(ChromeTheme::Light, false);
        assert!(
            channel_delta(active.fill, inactive.fill) >= 25.0,
            "tab fills"
        );
        assert!(active.rule_height >= 3.0, "rule thicker than hairline");
        let rule = active.rule.expect("active tab rule");
        for surface in [active.fill, inactive.fill, LIGHT.chrome] {
            assert!(contrast(rule, surface) >= 3.0, "rule on {surface:?}");
        }
        assert_eq!(inactive.rule, None);
        for cue in [active, inactive] {
            // Inactive labels stay full-strength text, never disabled grey.
            assert!(contrast(LIGHT.text, cue.fill) >= 4.5, "tab label");
            assert!(contrast(LIGHT.muted, cue.fill) >= 4.5, "close glyph");
        }
    }

    #[test]
    fn light_focused_pane_header_is_tinted_without_height_change() {
        let focused = pane_header_cue(ChromeTheme::Light, true);
        let unfocused = pane_header_cue(ChromeTheme::Light, false);
        assert_eq!(focused.rule_height, unfocused.rule_height);
        assert_eq!(focused.rule_height, 2.0);
        assert_ne!(focused.fill, unfocused.fill, "focused header tint");
        assert!(
            (focused.fill.r - unfocused.fill.r).abs() * 255.0 >= 20.0,
            "tint is perceptible"
        );
        let rule = focused.rule.expect("focus rule");
        assert_eq!(rule, LIGHT.selection_indicator);
        for surface in [focused.fill, unfocused.fill] {
            assert!(contrast(rule, surface) >= 3.0, "focus rule on {surface:?}");
        }
        for cue in [focused, unfocused] {
            assert!(contrast(LIGHT.text, cue.fill) >= 4.5, "pane title");
        }
        assert_eq!(unfocused.rule, None);
    }

    #[test]
    fn dark_state_cues_are_unchanged_and_readable() {
        let d = ChromeTheme::Dark;
        assert_eq!(menu_title_cue(d, true).fill, DARK.hover);
        assert_eq!(menu_title_cue(d, true).rule, None);
        assert_eq!(menu_item_hover_fill(d), DARK.hover);
        assert_eq!(session_tab_cue(d, true).rule, Some(DARK.accent));
        assert_eq!(session_tab_cue(d, true).rule_height, 2.0);
        assert_eq!(session_tab_cue(d, true).fill, DARK.tab_active);
        assert_eq!(pane_header_cue(d, true).fill, DARK.header);
        assert_eq!(pane_header_cue(d, true).rule, Some(DARK.accent));
        for open in [true, false] {
            for cue in [
                menu_title_cue(d, open),
                session_tab_cue(d, open),
                pane_header_cue(d, open),
            ] {
                assert!(contrast(DARK.text, cue.fill) >= 4.5, "{cue:?}");
            }
        }
    }

    #[test]
    fn status_bar_stays_white_on_windterm_blue() {
        for p in [LIGHT, DARK] {
            assert_eq!(p.status_bg, hex(0x007ACC));
            assert_eq!(p.status_text, hex(0xFFFFFF));
            assert!(contrast(p.status_text, p.status_bg) >= 4.5);
        }
    }
}
