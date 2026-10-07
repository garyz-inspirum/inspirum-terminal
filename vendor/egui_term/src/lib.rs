mod backend;
mod bindings;
mod font;
mod theme;
mod types;
mod view;

pub use backend::settings::BackendSettings;
pub use backend::{
    serialize_windows_program, BackendCommand, DisplayCell, DisplaySnapshot, MouseButton,
    MouseModifiers, PtyEvent, SelectionType, TerminalBackend, TerminalMode,
};
pub use bindings::{Binding, BindingAction, InputKind, KeyboardBinding};
pub use font::{FontSettings, TerminalFont};
pub use theme::{ColorPalette, TerminalTheme};
pub use types::Size;
pub use view::{CursorStyle, InteractionSettings, TerminalView};
