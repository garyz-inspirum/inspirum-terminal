# Keyboard and accessibility

The only frontend is Iced. Shortcut dispatch respects modal dialogs and captured form/editor input; hovering a terminal does not transfer keyboard ownership from a form.

Use the command palette and visible menu controls for connection, tab switching, history search, closing the active tab and explicitly arming synchronized input. Ctrl/Command+comma opens Tools. While Tools is open, Ctrl/Command+1 through 8 selects Profiles, Workspace, History, Appearance, SSH, SCP, Diagnostics and Snippets. Escape dismisses dialogs through their normal cancellation path. Clipboard writes and multiline remote commands retain explicit preview/confirmation guards.

Terminal key encoding honors application cursor mode, xterm modifiers and committed Unicode text. IME candidate/preedit UI belongs to Iced/winit and the native platform; only committed text is sent to the PTY. Tests cover native Iced preedit/commit events, cancellation when focus changes, and CJK/dead-key/emoji encoding. Native composition and candidate-window behavior, focus restoration and full-screen applications still need platform acceptance, especially macOS issue #78.

The application does not claim a verified screen-reader experience or complete accessibility tree. Keyboard navigation and scaling have automated coverage, but native screen-reader behavior, high-DPI layouts and platform accessibility acceptance require manual validation. GUI screenshots and successful builds are not substitutes for these checks.
