mod backend;
mod theme;
mod types;

pub use backend::settings::BackendSettings;
pub use backend::{
    serialize_windows_program, BackendCommand, DisplayCell, DisplaySnapshot, MouseButton,
    MouseModifiers, PtyEvent, SelectionType, TerminalBackend, TerminalMode, TerminalSize,
};
pub use theme::{ColorPalette, RgbColor, TerminalTheme};
pub use types::Size;
