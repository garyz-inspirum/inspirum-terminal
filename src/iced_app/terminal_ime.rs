//! Native IME routing around the complete UI so focused form editors take priority.
use super::Message;
use iced::advanced::input_method;
use iced::advanced::{Clipboard, Layout, Shell, Widget, layout, mouse, overlay, renderer, widget};
use iced::{Element, Event, Length, Rectangle, Renderer, Size, Theme, Vector};

#[derive(Default)]
struct State {
    target: Option<u64>,
    owner: Option<u64>,
    preedit: Option<input_method::Preedit>,
    reset: bool,
    waiting_for_open: bool,
    // Redraw events report the focused text input/editor via its IME request.
    // Keep this across key events: a text input can ignore Enter/Escape even
    // while it owns keyboard focus, and those keys must never reach the PTY.
    editor_has_focus: bool,
}
impl State {
    fn focus(&mut self, target: Option<u64>) {
        if self.target != target {
            self.target = target;
            self.owner = None;
            self.preedit = None;
            self.reset = true;
            self.waiting_for_open = true;
        }
    }
    fn ime(&mut self, event: &input_method::Event) -> Option<(u64, String)> {
        let target = self.target?;
        match event {
            input_method::Event::Opened => {
                self.owner = Some(target);
                self.waiting_for_open = false;
            }
            input_method::Event::Closed => {
                self.owner = None;
                self.preedit = None;
                self.waiting_for_open = true;
            }
            input_method::Event::Preedit(content, selection) if !self.waiting_for_open => {
                self.preedit = Some(input_method::Preedit {
                    content: content.clone(),
                    selection: selection.clone(),
                    text_size: None,
                });
            }
            input_method::Event::Commit(text)
                if !self.waiting_for_open && self.owner == Some(target) =>
            {
                self.preedit = None;
                return (!text.is_empty()).then(|| (target, text.clone()));
            }
            _ => {}
        }
        None
    }
    fn editor_owns_input(
        &mut self,
        redraw: bool,
        editor_ime_requested: bool,
        child_captured_text_event: bool,
    ) -> bool {
        // A text field requests an input method on redraw, not necessarily
        // on KeyPressed. Preserve the result until the next redraw so a
        // noncaptured Enter/Escape cannot escape into an SSH session.
        if redraw {
            self.editor_has_focus = editor_ime_requested;
        }
        self.editor_has_focus || editor_ime_requested || child_captured_text_event
    }
    fn composing(&self) -> bool {
        self.preedit.as_ref().is_some_and(|p| !p.content.is_empty())
    }
}

struct TerminalIme<'a> {
    content: Element<'a, Message>,
    target: Option<u64>,
    cursor: Option<Rectangle>,
}

pub(super) fn wrap<'a>(
    content: Element<'a, Message>,
    target: Option<u64>,
    cursor: Option<Rectangle>,
) -> Element<'a, Message> {
    Element::new(TerminalIme {
        content,
        target,
        cursor,
    })
}
impl Widget<Message, Theme, Renderer> for TerminalIme<'_> {
    fn size(&self) -> Size<Length> {
        self.content.as_widget().size()
    }
    fn size_hint(&self) -> Size<Length> {
        self.content.as_widget().size_hint()
    }
    fn tag(&self) -> widget::tree::Tag {
        widget::tree::Tag::of::<State>()
    }
    fn state(&self) -> widget::tree::State {
        widget::tree::State::new(State::default())
    }
    fn children(&self) -> Vec<widget::Tree> {
        vec![widget::Tree::new(&self.content)]
    }
    fn diff(&self, tree: &mut widget::Tree) {
        tree.diff_children(std::slice::from_ref(&self.content));
    }
    fn layout(
        &mut self,
        tree: &mut widget::Tree,
        renderer: &Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        self.content
            .as_widget_mut()
            .layout(&mut tree.children[0], renderer, limits)
    }
    fn draw(
        &self,
        tree: &widget::Tree,
        renderer: &mut Renderer,
        theme: &Theme,
        style: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        self.content.as_widget().draw(
            &tree.children[0],
            renderer,
            theme,
            style,
            layout,
            cursor,
            viewport,
        );
    }
    fn operate(
        &mut self,
        tree: &mut widget::Tree,
        layout: Layout<'_>,
        renderer: &Renderer,
        operation: &mut dyn widget::Operation,
    ) {
        self.content
            .as_widget_mut()
            .operate(&mut tree.children[0], layout, renderer, operation);
    }
    fn update(
        &mut self,
        tree: &mut widget::Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        self.content.as_widget_mut().update(
            &mut tree.children[0],
            event,
            layout,
            cursor,
            renderer,
            clipboard,
            shell,
            viewport,
        );
        let redraw = matches!(
            event,
            Event::Window(iced::window::Event::RedrawRequested(_))
        );
        let editor_ime_requested = shell.input_method().is_enabled();
        let child_captured_text_event = shell.is_event_captured()
            && matches!(event, Event::InputMethod(_) | Event::Keyboard(_));
        let state = tree.state.downcast_mut::<State>();
        let editor_owns_input =
            state.editor_owns_input(redraw, editor_ime_requested, child_captured_text_event);
        state.focus(if editor_owns_input { None } else { self.target });
        if editor_owns_input {
            // A focused editor may deliberately ignore a key (notably Enter
            // without on_submit). Do not let Iced's global ignored-event
            // subscription reinterpret it as terminal input.
            if matches!(event, Event::Keyboard(_) | Event::InputMethod(_)) {
                shell.capture_event();
            }
            return;
        }
        if self.target.is_none() {
            return;
        }
        match event {
            Event::InputMethod(event) => {
                if let Some((target, text)) = state.ime(event) {
                    shell.publish(Message::TerminalImeCommit(target, text));
                }
                shell.capture_event();
                shell.request_redraw();
            }
            Event::Keyboard(iced::keyboard::Event::KeyPressed { .. }) if state.composing() => {
                shell.capture_event();
            }
            _ => {}
        }
        if redraw {
            if state.reset {
                // Disable for one frame to cancel a composition owned by the prior pane.
                state.reset = false;
                shell.request_redraw();
            } else {
                shell.request_input_method(&input_method::InputMethod::Enabled {
                    cursor: self.cursor.unwrap_or_else(|| {
                        Rectangle::new(layout.bounds().position(), Size::new(1.0, 18.0))
                    }),
                    purpose: input_method::Purpose::Terminal,
                    preedit: state.preedit.clone(),
                });
            }
        }
    }
    fn mouse_interaction(
        &self,
        tree: &widget::Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &Renderer,
    ) -> mouse::Interaction {
        self.content.as_widget().mouse_interaction(
            &tree.children[0],
            layout,
            cursor,
            viewport,
            renderer,
        )
    }
    fn overlay<'a>(
        &'a mut self,
        tree: &'a mut widget::Tree,
        layout: Layout<'a>,
        renderer: &Renderer,
        viewport: &Rectangle,
        translation: Vector,
    ) -> Option<overlay::Element<'a, Message, Theme, Renderer>> {
        self.content.as_widget_mut().overlay(
            &mut tree.children[0],
            layout,
            renderer,
            viewport,
            translation,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn focused_text_field_blocks_unhandled_keys_until_blurred() {
        let mut state = State::default();
        state.focus(Some(7));
        assert!(!state.editor_owns_input(true, false, false));
        // Clicking the session search and drawing it makes the text editor
        // the IME owner. Enter can then be ignored by the TextInput widget.
        assert!(state.editor_owns_input(true, true, false));
        state.focus(None);
        assert!(state.editor_owns_input(false, false, false));
        assert!(state.editor_owns_input(false, false, true));
        // After a click on the terminal, the next redraw restores PTY input.
        assert!(!state.editor_owns_input(true, false, false));
        state.focus(Some(7));
        assert_eq!(state.target, Some(7));
    }
    #[test]
    fn native_preedit_never_sends_and_commit_uses_its_original_pane() {
        let mut state = State::default();
        state.focus(Some(7));
        assert_eq!(state.ime(&input_method::Event::Opened), None);
        assert_eq!(
            state.ime(&input_method::Event::Preedit("に".into(), Some(0..3))),
            None
        );
        assert!(state.composing());
        assert_eq!(
            state.ime(&input_method::Event::Commit("日本語".into())),
            Some((7, "日本語".into()))
        );
        assert!(!state.composing());
    }
    #[test]
    fn focus_change_or_editor_capture_cancels_pending_native_commit() {
        let mut state = State::default();
        state.focus(Some(7));
        state.ime(&input_method::Event::Opened);
        state.ime(&input_method::Event::Preedit("に".into(), None));
        state.focus(Some(8));
        state.ime(&input_method::Event::Preedit(String::new(), None));
        assert_eq!(
            state.ime(&input_method::Event::Commit("日本語".into())),
            None
        );
        state.ime(&input_method::Event::Opened);
        assert_eq!(
            state.ime(&input_method::Event::Commit("語".into())),
            Some((8, "語".into()))
        );
        state.focus(None);
        assert_eq!(
            state.ime(&input_method::Event::Commit("editor".into())),
            None
        );
    }
}
