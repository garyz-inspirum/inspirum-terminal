//! Pure dock layout model (spec section 5): which panels live in which dock
//! area, which tab is active, collapse state and sizes. No Iced widgets here.

use serde::{Deserialize, Serialize};

use super::theme::marker;

pub(super) const DEFAULT_SIDE_WIDTH: f32 = 214.0;
pub(super) const MIN_SIDE_WIDTH: f32 = 140.0;
pub(super) const MAX_SIDE_WIDTH: f32 = 900.0;
/// The centre workspace keeps at least this width when a side dock is dragged.
pub(super) const MIN_CENTRE_WIDTH: f32 = 320.0;
/// File tables (Filer/Explorer) are unusable in a narrower side dock.
pub(super) const FILE_PANEL_MIN_WIDTH: f32 = 420.0;
/// Width of the invisible splitter grab zone around the 1px border line.
pub(super) const SPLITTER_GRAB: f32 = 5.0;
pub(super) const DEFAULT_BOTTOM_RATIO: f32 = 0.32;
pub(super) const MIN_BOTTOM_RATIO: f32 = 0.15;
pub(super) const MAX_BOTTOM_RATIO: f32 = 0.7;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Panel {
    Sender,
    Session,
    Outline,
    Transfer,
    Filer,
    Explorer,
    Shell,
}

impl Panel {
    pub(super) const ALL: [Panel; 7] = [
        Panel::Sender,
        Panel::Session,
        Panel::Outline,
        Panel::Transfer,
        Panel::Filer,
        Panel::Explorer,
        Panel::Shell,
    ];

    pub(super) fn title(self) -> &'static str {
        match self {
            Panel::Sender => "Sender",
            Panel::Session => "Session",
            Panel::Outline => "Outline",
            Panel::Transfer => "Transfer",
            Panel::Filer => "Filer",
            Panel::Explorer => "Explorer",
            Panel::Shell => "Shell",
        }
    }

    pub(super) fn marker(self) -> iced::Color {
        match self {
            Panel::Sender => marker::SENDER,
            Panel::Session => marker::SESSION,
            Panel::Outline => marker::OUTLINE,
            Panel::Transfer => marker::TRANSFER,
            Panel::Filer => marker::FILER,
            Panel::Explorer => marker::EXPLORER,
            Panel::Shell => marker::SHELL,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Area {
    Left,
    Right,
    Bottom,
}

impl Area {
    pub(super) const ALL: [Area; 3] = [Area::Left, Area::Right, Area::Bottom];
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub(super) struct AreaState {
    pub panels: Vec<Panel>,
    pub active: usize,
    pub collapsed: bool,
}

impl AreaState {
    fn with(panels: Vec<Panel>, active: Panel) -> Self {
        let active = panels.iter().position(|p| *p == active).unwrap_or(0);
        Self {
            panels,
            active,
            collapsed: false,
        }
    }

    fn active_panel(&self) -> Option<Panel> {
        self.panels.get(self.active).copied()
    }

    fn fix_active(&mut self) {
        if self.active >= self.panels.len() {
            self.active = self.panels.len().saturating_sub(1);
        }
    }

    /// Remove `panel`; returns true if it was present.
    fn remove(&mut self, panel: Panel) -> bool {
        let Some(index) = self.panels.iter().position(|p| *p == panel) else {
            return false;
        };
        self.panels.remove(index);
        if index < self.active {
            self.active -= 1;
        }
        self.fix_active();
        true
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Preset {
    BottomTabs,
    WindTermDefault,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub(super) struct DockLayout {
    pub left: AreaState,
    pub right: AreaState,
    pub bottom: AreaState,
    /// Closed panels and the area to restore when they are shown again.
    pub hidden: Vec<(Panel, Area)>,
    pub left_width: f32,
    pub right_width: f32,
    pub bottom_ratio: f32,
}

impl Default for DockLayout {
    fn default() -> Self {
        Self::preset(Preset::BottomTabs)
    }
}

fn clamp_or(value: f32, min: f32, max: f32, default: f32) -> f32 {
    if value.is_finite() {
        value.clamp(min, max)
    } else {
        default
    }
}

impl DockLayout {
    pub(super) fn preset(preset: Preset) -> DockLayout {
        let (left, right, bottom) = match preset {
            Preset::BottomTabs => (
                AreaState::default(),
                AreaState::default(),
                AreaState::with(Panel::ALL.to_vec(), Panel::Session),
            ),
            Preset::WindTermDefault => (
                AreaState::with(vec![Panel::Filer, Panel::Explorer], Panel::Filer),
                AreaState::with(vec![Panel::Session, Panel::Outline], Panel::Session),
                AreaState::with(
                    vec![Panel::Sender, Panel::Transfer, Panel::Shell],
                    Panel::Sender,
                ),
            ),
        };
        DockLayout {
            left,
            right,
            bottom,
            hidden: Vec::new(),
            left_width: DEFAULT_SIDE_WIDTH,
            right_width: DEFAULT_SIDE_WIDTH,
            bottom_ratio: DEFAULT_BOTTOM_RATIO,
        }
    }

    pub(super) fn area(&self, area: Area) -> &AreaState {
        match area {
            Area::Left => &self.left,
            Area::Right => &self.right,
            Area::Bottom => &self.bottom,
        }
    }

    fn area_mut(&mut self, area: Area) -> &mut AreaState {
        match area {
            Area::Left => &mut self.left,
            Area::Right => &mut self.right,
            Area::Bottom => &mut self.bottom,
        }
    }

    pub(super) fn area_of(&self, panel: Panel) -> Option<Area> {
        Area::ALL
            .into_iter()
            .find(|area| self.area(*area).panels.contains(&panel))
    }

    pub(super) fn is_visible(&self, panel: Panel) -> bool {
        self.area_of(panel).is_some_and(|area| {
            let state = self.area(area);
            !state.collapsed && state.active_panel() == Some(panel)
        })
    }

    pub(super) fn active_panel(&self, area: Area) -> Option<Panel> {
        self.area(area).active_panel()
    }

    pub(super) fn activate(&mut self, panel: Panel) {
        let Some(area) = self.area_of(panel) else {
            return;
        };
        let state = self.area_mut(area);
        if let Some(index) = state.panels.iter().position(|p| *p == panel) {
            state.active = index;
            state.collapsed = false;
        }
    }

    pub(super) fn show(&mut self, panel: Panel) {
        if self.area_of(panel).is_none() {
            let area = self
                .hidden
                .iter()
                .position(|(p, _)| *p == panel)
                .map(|index| self.hidden.remove(index).1)
                .unwrap_or(Area::Bottom);
            self.area_mut(area).panels.push(panel);
        }
        self.activate(panel);
    }

    pub(super) fn close(&mut self, panel: Panel) {
        if let Some(area) = self.area_of(panel) {
            self.hidden.retain(|(p, _)| *p != panel);
            self.hidden.push((panel, area));
        }
        for area in Area::ALL {
            self.area_mut(area).remove(panel);
        }
    }

    pub(super) fn toggle(&mut self, panel: Panel) {
        if self.is_visible(panel) {
            self.close(panel);
        } else {
            self.show(panel);
        }
    }

    pub(super) fn move_to(&mut self, panel: Panel, target: Area) {
        self.close(panel);
        self.hidden.retain(|(p, _)| *p != panel);
        self.area_mut(target).panels.push(panel);
        self.activate(panel);
    }

    pub(super) fn toggle_collapsed(&mut self, area: Area) {
        let state = self.area_mut(area);
        state.collapsed = !state.collapsed;
    }

    pub(super) fn set_width(&mut self, area: Area, width: f32) {
        let width = clamp_or(width, MIN_SIDE_WIDTH, MAX_SIDE_WIDTH, DEFAULT_SIDE_WIDTH);
        match area {
            Area::Left => self.left_width = width,
            Area::Right => self.right_width = width,
            Area::Bottom => {}
        }
    }

    /// Ensure a side dock receiving a file panel is wide enough for its table.
    pub(super) fn widen_for_file_panel(&mut self, panel: Panel, area: Area) {
        if !matches!(panel, Panel::Filer | Panel::Explorer) {
            return;
        }
        match area {
            Area::Left if self.left_width < FILE_PANEL_MIN_WIDTH => {
                self.set_width(Area::Left, FILE_PANEL_MIN_WIDTH)
            }
            Area::Right if self.right_width < FILE_PANEL_MIN_WIDTH => {
                self.set_width(Area::Right, FILE_PANEL_MIN_WIDTH)
            }
            _ => {}
        }
    }

    /// Apply a splitter drag: `position` is the cursor in logical window
    /// coordinates, `window` the logical window size.
    pub(super) fn drag_splitter(&mut self, area: Area, position: iced::Point, window: iced::Size) {
        use super::theme::metrics::{MENU_BAR_HEIGHT, STATUS_BAR_HEIGHT};
        match area {
            Area::Left | Area::Right => {
                let wanted = if area == Area::Left {
                    position.x
                } else {
                    window.width - position.x
                };
                let other = if area == Area::Left {
                    if self.right.panels.is_empty() {
                        0.0
                    } else {
                        self.right_width
                    }
                } else if self.left.panels.is_empty() {
                    0.0
                } else {
                    self.left_width
                };
                let room = (window.width - other - MIN_CENTRE_WIDTH).max(MIN_SIDE_WIDTH);
                self.set_width(area, wanted.min(room));
            }
            Area::Bottom => {
                let available = window.height - MENU_BAR_HEIGHT - STATUS_BAR_HEIGHT;
                if available > 1.0 {
                    self.set_bottom_ratio(
                        (window.height - STATUS_BAR_HEIGHT - position.y) / available,
                    );
                }
            }
        }
    }

    pub(super) fn set_bottom_ratio(&mut self, ratio: f32) {
        self.bottom_ratio = clamp_or(
            ratio,
            MIN_BOTTOM_RATIO,
            MAX_BOTTOM_RATIO,
            DEFAULT_BOTTOM_RATIO,
        );
    }

    /// Drop panels that are not available (e.g. Outline without a data source).
    pub(super) fn retain_available(&mut self, available: &[Panel]) {
        for panel in Panel::ALL {
            if !available.contains(&panel) {
                for area in Area::ALL {
                    self.area_mut(area).remove(panel);
                }
            }
        }
    }

    /// Append panels absent from both the saved areas and closed-panel history
    /// (new panels introduced since the layout was saved), without changing the active tab.
    pub(super) fn add_missing(&mut self, available: &[Panel]) {
        for panel in available {
            let present = Area::ALL
                .iter()
                .any(|area| self.area(*area).panels.contains(panel));
            if !present && !self.hidden.iter().any(|(p, _)| p == panel) {
                let bottom = self.area_mut(Area::Bottom);
                let active = bottom.active_panel();
                bottom.panels.push(*panel);
                bottom.active = active
                    .and_then(|a| bottom.panels.iter().position(|p| *p == a))
                    .unwrap_or(0);
            }
        }
    }

    /// Enforce invariants after deserialising untrusted input.
    pub(super) fn sanitize(&mut self) {
        let mut seen: Vec<Panel> = Vec::new();
        for area in Area::ALL {
            let state = self.area_mut(area);
            let active = state.active_panel();
            state.panels.retain(|panel| {
                if seen.contains(panel) {
                    false
                } else {
                    seen.push(*panel);
                    true
                }
            });
            state.active = active
                .and_then(|a| state.panels.iter().position(|p| *p == a))
                .unwrap_or(0);
            state.fix_active();
        }
        self.hidden.retain(|(panel, _)| {
            if seen.contains(panel) {
                false
            } else {
                seen.push(*panel);
                true
            }
        });
        self.left_width = clamp_or(
            self.left_width,
            MIN_SIDE_WIDTH,
            MAX_SIDE_WIDTH,
            DEFAULT_SIDE_WIDTH,
        );
        self.right_width = clamp_or(
            self.right_width,
            MIN_SIDE_WIDTH,
            MAX_SIDE_WIDTH,
            DEFAULT_SIDE_WIDTH,
        );
        self.set_bottom_ratio(self.bottom_ratio);
    }

    #[cfg_attr(not(test), allow(dead_code))] // used by tests
    pub(super) fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }

    #[cfg_attr(not(test), allow(dead_code))] // used by tests
    pub(super) fn from_json(json: &str) -> DockLayout {
        let mut layout: DockLayout = serde_json::from_str(json).unwrap_or_default();
        layout.sanitize();
        layout
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_invariants(layout: &DockLayout) {
        let mut seen = Vec::new();
        for area in Area::ALL {
            let state = layout.area(area);
            for panel in &state.panels {
                assert!(!seen.contains(panel), "{panel:?} appears twice");
                seen.push(*panel);
            }
            if state.panels.is_empty() {
                assert_eq!(state.active, 0);
            } else {
                assert!(state.active < state.panels.len());
            }
        }
        assert!((MIN_SIDE_WIDTH..=MAX_SIDE_WIDTH).contains(&layout.left_width));
        assert!((MIN_SIDE_WIDTH..=MAX_SIDE_WIDTH).contains(&layout.right_width));
        assert!((MIN_BOTTOM_RATIO..=MAX_BOTTOM_RATIO).contains(&layout.bottom_ratio));
    }

    #[test]
    fn dock_presets_satisfy_invariants() {
        let bottom = DockLayout::preset(Preset::BottomTabs);
        assert_invariants(&bottom);
        assert_eq!(bottom.bottom.panels, Panel::ALL.to_vec());
        assert_eq!(bottom.active_panel(Area::Bottom), Some(Panel::Session));
        assert!(bottom.left.panels.is_empty() && bottom.right.panels.is_empty());
        assert_eq!(DockLayout::default(), bottom);

        let wt = DockLayout::preset(Preset::WindTermDefault);
        assert_invariants(&wt);
        assert_eq!(wt.left.panels, vec![Panel::Filer, Panel::Explorer]);
        assert_eq!(wt.right.panels, vec![Panel::Session, Panel::Outline]);
        assert_eq!(
            wt.bottom.panels,
            vec![Panel::Sender, Panel::Transfer, Panel::Shell]
        );
    }

    #[test]
    fn dock_panel_titles_and_markers() {
        assert_eq!(Panel::Sender.title(), "Sender");
        assert_eq!(Panel::Explorer.title(), "Explorer");
        assert_eq!(Panel::Shell.marker(), marker::SHELL);
    }

    #[test]
    fn dock_move_keeps_panels_unique() {
        let mut layout = DockLayout::preset(Preset::WindTermDefault);
        layout.move_to(Panel::Shell, Area::Left);
        assert_invariants(&layout);
        assert_eq!(layout.area_of(Panel::Shell), Some(Area::Left));
        assert_eq!(layout.active_panel(Area::Left), Some(Panel::Shell));
        layout.move_to(Panel::Shell, Area::Left);
        assert_invariants(&layout);
        assert_eq!(layout.left.panels.len(), 3);
    }

    #[test]
    fn dock_close_active_selects_neighbour() {
        let mut layout = DockLayout::default();
        layout.activate(Panel::Shell);
        layout.close(Panel::Shell);
        assert_invariants(&layout);
        assert_eq!(layout.active_panel(Area::Bottom), Some(Panel::Explorer));
        layout.activate(Panel::Session);
        layout.close(Panel::Session);
        assert_eq!(layout.active_panel(Area::Bottom), Some(Panel::Outline));
        layout.close(Panel::Sender);
        assert_eq!(layout.active_panel(Area::Bottom), Some(Panel::Outline));
        assert_invariants(&layout);
    }

    #[test]
    fn dock_close_all_leaves_empty_area() {
        let mut layout = DockLayout::default();
        for panel in Panel::ALL {
            layout.close(panel);
        }
        assert_invariants(&layout);
        assert_eq!(layout.active_panel(Area::Bottom), None);
    }

    #[test]
    fn dock_show_adds_hidden_panel_to_bottom() {
        let mut layout = DockLayout::default();
        layout.close(Panel::Filer);
        layout.show(Panel::Filer);
        assert_invariants(&layout);
        assert_eq!(layout.bottom.panels.last(), Some(&Panel::Filer));
        assert!(layout.is_visible(Panel::Filer));
        layout.show(Panel::Filer);
        assert_invariants(&layout);
    }

    #[test]
    fn dock_toggle_closes_visible_and_shows_hidden() {
        let mut layout = DockLayout::default();
        assert!(layout.is_visible(Panel::Session));
        layout.toggle(Panel::Session);
        assert_eq!(layout.area_of(Panel::Session), None);
        layout.toggle(Panel::Session);
        assert!(layout.is_visible(Panel::Session));
        // Present but not active: toggle activates it.
        layout.toggle(Panel::Sender);
        assert!(layout.is_visible(Panel::Sender));
        assert_invariants(&layout);
    }

    #[test]
    fn dock_activate_uncollapses() {
        let mut layout = DockLayout::preset(Preset::WindTermDefault);
        layout.toggle_collapsed(Area::Right);
        assert!(!layout.is_visible(Panel::Session));
        layout.activate(Panel::Outline);
        assert!(!layout.right.collapsed);
        assert!(layout.is_visible(Panel::Outline));
    }

    #[test]
    fn file_panel_widens_side_dock() {
        let mut layout = DockLayout::default();
        layout.set_width(Area::Right, 214.0);
        layout.widen_for_file_panel(Panel::Filer, Area::Right);
        assert_eq!(layout.right_width, FILE_PANEL_MIN_WIDTH);
        layout.set_width(Area::Left, 500.0);
        layout.widen_for_file_panel(Panel::Explorer, Area::Left);
        assert_eq!(layout.left_width, 500.0);
        layout.set_width(Area::Left, 200.0);
        layout.widen_for_file_panel(Panel::Sender, Area::Left);
        assert_eq!(layout.left_width, 200.0);
    }

    #[test]
    fn splitter_drag_clamps_to_keep_centre() {
        let mut layout = DockLayout::default();
        let win = iced::Size::new(1280.0, 800.0);
        layout.drag_splitter(Area::Right, iced::Point::new(1280.0 - 500.0, 100.0), win);
        assert!((layout.right_width - 500.0).abs() < 1.0);
        layout.drag_splitter(Area::Right, iced::Point::new(0.0, 100.0), win);
        let left = if layout.left.panels.is_empty() {
            0.0
        } else {
            layout.left_width
        };
        assert!(layout.right_width <= 1280.0 - left - MIN_CENTRE_WIDTH + 0.5);
        layout.drag_splitter(Area::Bottom, iced::Point::new(10.0, 0.0), win);
        assert_eq!(layout.bottom_ratio, MAX_BOTTOM_RATIO);
        layout.drag_splitter(Area::Bottom, iced::Point::new(10.0, 799.0), win);
        assert_eq!(layout.bottom_ratio, MIN_BOTTOM_RATIO);
    }

    #[test]
    fn dock_sizes_are_clamped() {
        let mut layout = DockLayout::default();
        layout.set_width(Area::Left, 10.0);
        layout.set_width(Area::Right, 10_000.0);
        layout.set_bottom_ratio(0.99);
        assert_eq!(layout.left_width, MIN_SIDE_WIDTH);
        assert_eq!(layout.right_width, MAX_SIDE_WIDTH);
        assert_eq!(layout.bottom_ratio, MAX_BOTTOM_RATIO);
        layout.set_bottom_ratio(f32::NAN);
        assert_eq!(layout.bottom_ratio, DEFAULT_BOTTOM_RATIO);
    }

    #[test]
    fn dock_sanitize_dedupes_and_clamps() {
        let mut layout = DockLayout {
            left: AreaState {
                panels: vec![Panel::Shell, Panel::Filer, Panel::Shell],
                active: 9,
                collapsed: false,
            },
            right: AreaState {
                panels: vec![Panel::Filer, Panel::Outline],
                active: 1,
                collapsed: true,
            },
            bottom: AreaState::default(),
            hidden: Vec::new(),
            left_width: f32::NAN,
            right_width: 1.0,
            bottom_ratio: 5.0,
        };
        layout.sanitize();
        assert_invariants(&layout);
        assert_eq!(layout.left.panels, vec![Panel::Shell, Panel::Filer]);
        assert_eq!(layout.right.panels, vec![Panel::Outline]);
        assert_eq!(layout.active_panel(Area::Right), Some(Panel::Outline));
        assert_eq!(layout.left_width, DEFAULT_SIDE_WIDTH);
        assert_eq!(layout.right_width, MIN_SIDE_WIDTH);
        assert_eq!(layout.bottom_ratio, MAX_BOTTOM_RATIO);
    }

    #[test]
    fn dock_json_round_trip() {
        let mut layout = DockLayout::preset(Preset::WindTermDefault);
        layout.move_to(Panel::Outline, Area::Bottom);
        layout.toggle_collapsed(Area::Left);
        layout.set_width(Area::Right, 300.0);
        let json = layout.to_json();
        assert!(json.contains("\"outline\""));
        assert_eq!(DockLayout::from_json(&json), layout);
    }

    #[test]
    fn dock_garbage_json_gives_default() {
        assert_eq!(DockLayout::from_json("not json"), DockLayout::default());
        assert_eq!(
            DockLayout::from_json(r#"{"left":{"panels":["bogus"]}}"#),
            DockLayout::default()
        );
        let partial = DockLayout::from_json(r#"{"left_width": 1000}"#);
        assert_eq!(partial.left_width, MAX_SIDE_WIDTH);
        assert_eq!(partial.bottom, DockLayout::default().bottom);
    }

    #[test]
    fn dock_retain_available_drops_missing() {
        let mut layout = DockLayout::preset(Preset::WindTermDefault);
        let available: Vec<Panel> = Panel::ALL
            .into_iter()
            .filter(|p| *p != Panel::Outline)
            .collect();
        layout.retain_available(&available);
        assert_invariants(&layout);
        assert_eq!(layout.area_of(Panel::Outline), None);
        assert_eq!(layout.right.panels, vec![Panel::Session]);
    }

    #[test]
    fn dock_add_missing_appends_without_changing_active() {
        let mut layout = DockLayout::preset(Preset::BottomTabs);
        layout.close(Panel::Sender);
        layout.activate(Panel::Session);
        layout.add_missing(&[Panel::Sender, Panel::Session, Panel::Filer]);
        assert_invariants(&layout);
        assert_eq!(
            layout.area_of(Panel::Sender),
            None,
            "closed panels are not missing"
        );
        assert_eq!(layout.bottom.active_panel(), Some(Panel::Session));
        let before = layout.clone();
        layout.add_missing(&[Panel::Sender]);
        assert_eq!(layout, before);
    }

    #[test]
    fn dock_saved_hidden_panel_stays_hidden_and_reopens_in_last_area() {
        use super::super::ui_state::{self, UiState};

        let path = std::env::temp_dir().join(format!(
            "inspirum-dock-hidden-{}-ui-state.json",
            std::process::id()
        ));
        let mut layout = DockLayout::default();
        layout.move_to(Panel::Filer, Area::Right);
        layout.close(Panel::Filer);
        ui_state::save(
            &path,
            &UiState {
                theme: String::new(),
                docks: Some(layout),
                external_editor: String::new(),
            },
        )
        .unwrap();
        let mut restored = ui_state::load(&path).docks.unwrap();
        std::fs::remove_file(&path).unwrap();
        restored.sanitize();
        restored.add_missing(&Panel::ALL);
        assert_eq!(restored.area_of(Panel::Filer), None);
        restored.show(Panel::Filer);
        assert_eq!(restored.area_of(Panel::Filer), Some(Area::Right));
        assert!(restored.is_visible(Panel::Filer));
    }

    #[test]
    fn dock_legacy_layout_adds_new_sender() {
        let mut old = DockLayout::default();
        old.bottom.remove(Panel::Sender); // legacy snapshot predating Sender
        let mut restored = DockLayout::from_json(&old.to_json());
        restored.add_missing(&Panel::ALL);
        assert_eq!(restored.area_of(Panel::Sender), Some(Area::Bottom));
        assert_eq!(restored.active_panel(Area::Bottom), Some(Panel::Session));
    }

    #[test]
    fn dock_retain_available_keeps_active_panel() {
        let mut layout = DockLayout::preset(Preset::BottomTabs);
        layout.retain_available(&[Panel::Sender, Panel::Session, Panel::Filer]);
        assert_eq!(layout.bottom.active_panel(), Some(Panel::Session));
    }
}
