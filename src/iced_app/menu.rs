//! WindTerm-style menu bar data: titles, entries and the commands they map to.

use super::dock::Panel as DockPanel;
use super::tools::Panel;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Menu {
    Session,
    Edit,
    View,
    Mode,
    Window,
    Help,
}

impl Menu {
    pub(super) const ALL: [Menu; 6] = [
        Menu::Session,
        Menu::Edit,
        Menu::View,
        Menu::Mode,
        Menu::Window,
        Menu::Help,
    ];

    pub(super) fn title(self) -> &'static str {
        match self {
            Menu::Session => "Session",
            Menu::Edit => "Edit",
            Menu::View => "View",
            Menu::Mode => "Mode",
            Menu::Window => "Window",
            Menu::Help => "Help",
        }
    }

    /// Estimated rendered width of the title button (13 px monospace + padding).
    pub(super) fn title_width(self) -> f32 {
        self.title().chars().count() as f32 * 7.8 + 16.0
    }

    /// Left offset of this menu's title within the menu bar.
    pub(super) fn offset(self) -> f32 {
        Menu::ALL
            .iter()
            .take_while(|menu| **menu != self)
            .map(|menu| menu.title_width())
            .sum()
    }

    pub(super) fn entries(self) -> Vec<Entry> {
        use Command::*;
        let item = |label, shortcut, command| Entry::Item {
            label,
            shortcut,
            command,
        };
        match self {
            Menu::Session => vec![
                item("New Session", "Ctrl+N", NewSession),
                item("Import Profiles", "", ImportProfiles),
                Entry::Separator,
                item("Tunnels", "", Tunnels),
                Entry::Separator,
                item("Edit Session", "", EditSession),
                item("Reconnect", "", Reconnect),
                item("Close Session", "", CloseSession),
                Entry::Separator,
                item("Lock Screen", "Ctrl+Shift+L", LockScreen),
                Entry::Separator,
                item("Exit", "", Exit),
            ],
            Menu::Edit => vec![
                item("Copy", "", Copy),
                item("Paste", "", Paste),
                item("Find", "Ctrl+Shift+F", Find),
                Entry::Separator,
                item("Command Palette", "", Palette),
                item("Snippets", "", Tool(Panel::Snippets)),
            ],
            Menu::View => vec![
                item("Theme: Light", "", ThemeLight),
                item("Theme: Dark", "", ThemeDark),
                Entry::Separator,
                item("Zoom In", "", ZoomIn),
                item("Zoom Out", "", ZoomOut),
                item("Focus Mode", "Alt+Enter", FocusMode),
                Entry::Separator,
                item("Session", "", ToggleSessions),
                item("Sender", "", TogglePanel(DockPanel::Sender)),
                item("Transfer", "", TogglePanel(DockPanel::Transfer)),
                item("Filer", "", TogglePanel(DockPanel::Filer)),
                item("Explorer", "", TogglePanel(DockPanel::Explorer)),
                item("Shell", "", ToggleShell),
            ],
            Menu::Mode => vec![
                item("Remote / Local Mode", "", LocalMode),
                item("Free Type Mode", "", FreeType),
                item("Remote Keys", "", RemoteKeys),
                Entry::Separator,
                item("Sync Input...", "", SyncInput),
            ],
            Menu::Window => vec![
                item("Layout: Bottom tabs", "", LayoutBottomTabs),
                item("Layout: WindTerm default", "", LayoutWindTerm),
                Entry::Separator,
                item("Split Right", "", SplitRight),
                item("Split Down", "", SplitDown),
                item("Close Pane", "", ClosePane),
                Entry::Separator,
                item("Next Tab", "", NextTab),
                item("Previous Tab", "", PreviousTab),
                Entry::Separator,
                item("Profiles", "", Tool(Panel::Profiles)),
                item("Workspace", "", Tool(Panel::Workspace)),
                item("History", "", Tool(Panel::History)),
                item("Appearance", "", Tool(Panel::Appearance)),
                item("SSH", "", Tool(Panel::Ssh)),
                item("SCP", "", Tool(Panel::Scp)),
                item("Diagnostics", "", Tool(Panel::Diagnostics)),
                item("Snippets", "", Tool(Panel::Snippets)),
            ],
            Menu::Help => vec![item("About", "", About)],
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Entry {
    Item {
        label: &'static str,
        shortcut: &'static str,
        command: Command,
    },
    Separator,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Command {
    NewSession,
    ImportProfiles,
    Tunnels,
    LockScreen,
    Paste,
    Find,
    Palette,
    ThemeLight,
    ThemeDark,
    ZoomIn,
    ZoomOut,
    FocusMode,
    ToggleSessions,
    TogglePanel(DockPanel),
    EditSession,
    Reconnect,
    CloseSession,
    Copy,
    SyncInput,
    ClosePane,
    ToggleShell,
    LocalMode,
    FreeType,
    RemoteKeys,
    SplitRight,
    SplitDown,
    NextTab,
    PreviousTab,
    Tool(Panel),
    About,
    LayoutBottomTabs,
    LayoutWindTerm,
    Exit,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn every_menu_has_items_and_no_edge_separators() {
        for menu in Menu::ALL {
            let entries = menu.entries();
            assert!(
                entries.iter().any(|e| matches!(e, Entry::Item { .. })),
                "{menu:?} has no items"
            );
            assert_ne!(entries.first(), Some(&Entry::Separator), "{menu:?}");
            assert_ne!(entries.last(), Some(&Entry::Separator), "{menu:?}");
        }
    }

    #[test]
    fn labels_unique_within_menu() {
        for menu in Menu::ALL {
            let mut seen = HashSet::new();
            for entry in menu.entries() {
                if let Entry::Item { label, .. } = entry {
                    assert!(seen.insert(label), "{menu:?} duplicates {label}");
                }
            }
        }
    }

    #[test]
    fn titles_follow_windterm_order() {
        let titles: Vec<_> = Menu::ALL.iter().map(|m| m.title()).collect();
        assert_eq!(
            titles,
            ["Session", "Edit", "View", "Mode", "Window", "Help"]
        );
        assert_eq!(Menu::Session.offset(), 0.0);
        assert!(Menu::Edit.offset() > 0.0);
    }

    #[test]
    fn edit_menu_find_follows_paste_with_shortcut() {
        let entries = Menu::Edit.entries();
        let paste = entries
            .iter()
            .position(|entry| matches!(entry, Entry::Item { label: "Paste", .. }));
        let Some(paste) = paste else {
            panic!("Edit menu has no Paste");
        };
        assert!(matches!(
            entries.get(paste + 1),
            Some(Entry::Item {
                label: "Find",
                shortcut: "Ctrl+Shift+F",
                command: Command::Find,
            })
        ));
    }

    #[test]
    fn session_menu_ends_with_ungrouped_exit() {
        let entries = Menu::Session.entries();
        assert!(matches!(
            entries.get(entries.len().saturating_sub(2)),
            Some(Entry::Separator)
        ));
        match entries.last() {
            Some(Entry::Item {
                label: "Exit",
                shortcut: "",
                command: Command::Exit,
            }) => {}
            other => panic!("Session menu must end with Exit and no shortcut, got {other:?}"),
        }
    }
}
