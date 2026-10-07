//! Cross-platform keyboard shortcut policy.
//!
//! Egui's `command` modifier maps to Command on macOS and Ctrl on Windows/Linux.
//! Destructive/navigation application shortcuts are suppressed while a confirmation
//! dialog is active so they cannot bypass guarded paste or other modal decisions.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShortcutKey {
    Enter,
    K,
    P,
    F,
    W,
    ArrowLeft,
    ArrowRight,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ShortcutModifiers {
    pub command: bool,
    pub shift: bool,
    pub alt: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShortcutAction {
    Connect,
    QuickSwitch,
    CommandPalette,
    Search,
    CloseActive,
    FocusPreviousPane,
    FocusNextPane,
}

pub fn action_for(
    key: ShortcutKey,
    modifiers: ShortcutModifiers,
    confirmation_open: bool,
) -> Option<ShortcutAction> {
    if confirmation_open {
        return None;
    }

    match (key, modifiers) {
        (
            ShortcutKey::Enter,
            ShortcutModifiers {
                command: true,
                shift: false,
                alt: false,
            },
        ) => Some(ShortcutAction::Connect),
        (
            ShortcutKey::K,
            ShortcutModifiers {
                command: true,
                shift: true,
                alt: false,
            },
        ) => Some(ShortcutAction::QuickSwitch),
        (
            ShortcutKey::P,
            ShortcutModifiers {
                command: true,
                shift: true,
                alt: false,
            },
        ) => Some(ShortcutAction::CommandPalette),
        (
            ShortcutKey::F,
            ShortcutModifiers {
                command: true,
                shift: true,
                alt: false,
            },
        ) => Some(ShortcutAction::Search),
        (
            ShortcutKey::W,
            ShortcutModifiers {
                command: true,
                shift: false,
                alt: false,
            },
        ) => Some(ShortcutAction::CloseActive),
        (
            ShortcutKey::ArrowLeft,
            ShortcutModifiers {
                command: true,
                shift: false,
                alt: true,
            },
        ) => Some(ShortcutAction::FocusPreviousPane),
        (
            ShortcutKey::ArrowRight,
            ShortcutModifiers {
                command: true,
                shift: false,
                alt: true,
            },
        ) => Some(ShortcutAction::FocusNextPane),
        _ => None,
    }
}

/// Convert committed text (including IME/dead-key composed Unicode) to PTY bytes.
///
/// Pre-edit/composition UI remains owned by egui/the native platform. Inspirum only sends
/// the final committed text event, so partially composed CJK/dead-key text is never written.
pub fn committed_text_bytes(text: &str) -> Option<Vec<u8>> {
    (!text.is_empty()).then(|| text.as_bytes().to_vec())
}

/// Move pane focus without mutating pane order.
pub fn adjacent_pane(panes: &[u64], current: Option<u64>, direction: isize) -> Option<u64> {
    if panes.is_empty() {
        return None;
    }
    let index = current
        .and_then(|id| panes.iter().position(|candidate| *candidate == id))
        .unwrap_or(0);

    let next = if direction < 0 {
        if index == 0 {
            panes.len() - 1
        } else {
            index - 1
        }
    } else {
        (index + 1) % panes.len()
    };
    Some(panes[next])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn command() -> ShortcutModifiers {
        ShortcutModifiers {
            command: true,
            ..ShortcutModifiers::default()
        }
    }

    #[test]
    fn required_keyboard_only_actions_have_unique_shortcuts() {
        assert_eq!(
            action_for(ShortcutKey::Enter, command(), false),
            Some(ShortcutAction::Connect)
        );
        assert_eq!(
            action_for(
                ShortcutKey::K,
                ShortcutModifiers {
                    command: true,
                    shift: true,
                    alt: false,
                },
                false,
            ),
            Some(ShortcutAction::QuickSwitch)
        );
        assert_eq!(
            action_for(
                ShortcutKey::P,
                ShortcutModifiers {
                    command: true,
                    shift: true,
                    alt: false,
                },
                false,
            ),
            Some(ShortcutAction::CommandPalette)
        );
        assert_eq!(
            action_for(
                ShortcutKey::F,
                ShortcutModifiers {
                    command: true,
                    shift: true,
                    alt: false,
                },
                false,
            ),
            Some(ShortcutAction::Search)
        );
        assert_eq!(
            action_for(ShortcutKey::W, command(), false),
            Some(ShortcutAction::CloseActive)
        );
        assert_eq!(
            action_for(
                ShortcutKey::ArrowRight,
                ShortcutModifiers {
                    command: true,
                    shift: false,
                    alt: true,
                },
                false,
            ),
            Some(ShortcutAction::FocusNextPane)
        );
    }

    #[test]
    fn modal_confirmation_blocks_all_application_shortcuts() {
        let candidates = [
            (ShortcutKey::Enter, command()),
            (
                ShortcutKey::K,
                ShortcutModifiers {
                    command: true,
                    shift: true,
                    alt: false,
                },
            ),
            (
                ShortcutKey::P,
                ShortcutModifiers {
                    command: true,
                    shift: true,
                    alt: false,
                },
            ),
            (
                ShortcutKey::F,
                ShortcutModifiers {
                    command: true,
                    shift: true,
                    alt: false,
                },
            ),
            (ShortcutKey::W, command()),
            (
                ShortcutKey::ArrowLeft,
                ShortcutModifiers {
                    command: true,
                    shift: false,
                    alt: true,
                },
            ),
        ];
        for (key, modifiers) in candidates {
            assert_eq!(action_for(key, modifiers, true), None);
        }
    }

    #[test]
    fn committed_unicode_text_preserves_cjk_dead_key_and_emoji_bytes() {
        for text in ["東京", "é", "e\u{301}", "🚀"] {
            assert_eq!(
                committed_text_bytes(text),
                Some(text.as_bytes().to_vec()),
                "{text:?}"
            );
        }
        assert_eq!(committed_text_bytes(""), None);
    }

    #[test]
    fn pane_focus_wraps_without_reordering() {
        let panes = [10, 20, 30];
        assert_eq!(adjacent_pane(&panes, Some(20), 1), Some(30));
        assert_eq!(adjacent_pane(&panes, Some(30), 1), Some(10));
        assert_eq!(adjacent_pane(&panes, Some(10), -1), Some(30));
        assert_eq!(panes, [10, 20, 30]);
    }
}
