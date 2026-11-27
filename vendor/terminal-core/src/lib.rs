mod backend;
mod theme;
mod types;

pub use backend::settings::BackendSettings;
pub use backend::{
    logical_lines_from_rows, probe_drag_selection, probe_drag_selection_steps,
    serialize_windows_program, BackendCommand, DisplayCell, DisplaySnapshot, DragProbeStep,
    DragSelectionProbe, HistoryCell, LogicalLine, MouseButton, MouseModifiers, PtyEvent,
    SelectionType, TerminalBackend, TerminalMode, TerminalSize, SCROLLBACK_LINES,
};
pub use theme::{ColorPalette, RgbColor, TerminalTheme};
pub use types::Size;
