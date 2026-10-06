# Keyboard, IME, focus and accessibility

Issue #59 audits the native terminal's keyboard-heavy and international-input paths.

## Keyboard-only workflows

Egui's **Command** modifier maps to **Cmd** on macOS and **Ctrl** on Windows/Linux.

| Action | Shortcut |
| --- | --- |
| Connect the current SSH draft | Ctrl/Cmd+Enter |
| Quick-switch open tabs | Ctrl/Cmd+Shift+K |
| Search retained terminal history | Ctrl/Cmd+Shift+F |
| Close the active terminal tab | Ctrl/Cmd+W |
| Focus previous split pane | Ctrl/Cmd+Alt+Left |
| Focus next split pane | Ctrl/Cmd+Alt+Right |
| Search result next / previous | F3 / Shift+F3 |
| Confirm / cancel guarded paste | Enter / Escape |

The quick switcher is keyboard navigable with Up/Down, Enter and Escape. Standard
egui Tab/Shift+Tab traversal remains available for ordinary form controls such as the
profile list, connection fields and buttons.

Application shortcuts are suppressed while an Inspirum confirmation is open. This
includes guarded paste, saved-profile delete, profile-library replacement, host-key
removal and bulk-tab close confirmations. A shortcut therefore cannot skip a modal
decision or turn a multiline paste into an unconfirmed PTY write.

## Focus behavior

The active terminal pane shows the existing **FOCUSED** marker. Split-focus shortcuts
change only active/focused pane identity; they do not reorder panes, reconnect sessions
or replay input. Closing the focused tab uses the same centralized PTY cleanup path as
mouse-driven close actions.

When a text field, quick switcher, search field or confirmation is active, terminal
keyboard focus is surrendered. Returning to a terminal restores explicit terminal focus
rather than sending form-navigation keystrokes to the PTY.

## IME and composed characters

Inspirum does not implement a private IME. It relies on egui/winit and the native window
system for composition/pre-edit UI. The terminal consumes only final committed text
events. Final UTF-8 text is forwarded unchanged, including CJK text, precomposed accented
characters, combining-character sequences and emoji.

This design means partially composed text is not intentionally written to the PTY.
Native candidate-window placement and pre-edit rendering remain platform/window-system
capabilities rather than terminal-emulator logic.

Platform limitations:

- **Windows x64:** committed native IME text is handled through egui text events. Candidate
  UI behavior is owned by the Windows/winit integration and is not controlled by Inspirum.
- **macOS Apple Silicon:** committed text and dead-key composition are handled through the
  native egui/winit text path. Candidate UI is platform-managed.
- **Linux x64:** committed text is supported, but IME pre-edit/candidate behavior can vary
  between Wayland/X11, desktop environment and input-method framework (for example IBus
  or Fcitx). Inspirum does not claim identical candidate-window behavior across those
  combinations.

CI exercises committed UTF-8 conversion on all supported native targets. OS IME candidate
windows are not meaningfully automatable in the current GitHub-hosted native CI runners;
that limitation is explicit rather than reported as fully automated coverage.

## Shortcut-conflict and paste-safety audit

Terminal control sequences remain unmodified when no application shortcut matches. The
new application shortcuts all require the cross-platform Command modifier, and split
focus additionally requires Alt. They do not overlap the terminal's ordinary unmodified
Arrow/Enter input.

Paste remains special: OS/keyboard paste events are intercepted before terminal delivery
and classified by the existing guarded-paste policy. While a guarded-paste confirmation
is open, application shortcuts are disabled and only the confirmation's own Enter/Escape
handling applies.

## Accessibility findings

Primary actions use visible text labels rather than icon-only controls where practical.
Connection fields have adjacent textual labels; search fields use descriptive hint text;
the split-pane focus state is visible in text; confirmation dialogs describe the affected
action and expose explicit confirm/cancel buttons. These structures are available to
egui's native accessibility integration where the platform/backend exposes it.

Remaining limitations:

- Inspirum does not currently provide a custom screen-reader tree beyond egui/AccessKit's
  generated semantics.
- Terminal cell contents are rendered by the terminal widget and are not yet exposed as
  a rich line-by-line screen-reader document model.
- Native screen-reader quality therefore depends on egui/AccessKit and OS support; full
  parity with platform-native terminal accessibility APIs is not claimed.
- Whole-application zoom/high-DPI scaling is delegated to egui's point/pixel scaling.
  Terminal cell measurement is recalculated from the active font and viewport size.
- OS IME candidate placement and screen-reader interaction require native manual testing
  beyond what headless CI can assert.

## Native smoke checklist

The release CI still builds/tests/packages Linux x64, Windows x64 and macOS Apple Silicon.
For a native GUI smoke, verify on each platform:

1. Ctrl/Cmd+Enter connects a valid draft through the normal host-key/authentication path.
2. Ctrl/Cmd+Shift+K switches tabs without sending shortcut bytes to the terminal.
3. Ctrl/Cmd+Alt+Left/Right visibly moves **FOCUSED** between split panes.
4. Ctrl/Cmd+Shift+F opens history search; F3 and Shift+F3 navigate results.
5. Ctrl/Cmd+W closes only the active tab through centralized cleanup.
6. A multiline paste still requires the configured confirmation when initiated from any
   supported input path.
7. CJK/dead-key input is committed once, with no partial pre-edit text intentionally sent
   to the PTY.
8. Tab/Shift+Tab can traverse ordinary connection-form controls without typing into an
   unfocused terminal.
