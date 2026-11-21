//! Dock area rendering (spec section 5): left/right/bottom areas with a
//! title or tab strip, gear menu and the active panel body.

use super::*;
use dock::{Area, Panel};
use iced::Length::FillPortion;
use iced::widget::{column, row};

const GEAR_MENU_WIDTH: f32 = 200.0;

/// Panels that have a body in this stage. Later stages add Filer, Shell, ...
pub(super) fn available_panels() -> Vec<dock::Panel> {
    vec![
        Panel::Sender,
        Panel::Session,
        Panel::Transfer,
        Panel::Filer,
        Panel::Explorer,
    ]
}

fn split_portions(ratio: f32) -> (u16, u16) {
    let r = ratio.clamp(0.0, 1.0);
    let b = ((r * 1000.0) as u16).max(1);
    let a = (((1.0 - r) * 1000.0) as u16).max(1);
    (a, b)
}

/// WindTerm-style splitter: a 1px border line centred in a transparent
/// `SPLITTER_GRAB`-wide (or high) grab zone with a resize cursor.
fn splitter<'a>(area: Area) -> Element<'a, Message> {
    let grab = dock::SPLITTER_GRAB;
    let (zone, cursor) = if area == Area::Bottom {
        (
            container(
                container(space::horizontal())
                    .width(Fill)
                    .height(1)
                    .style(border_fill),
            )
            .width(Fill)
            .height(grab)
            .center_y(grab),
            iced::mouse::Interaction::ResizingVertically,
        )
    } else {
        (
            container(
                container(space::horizontal())
                    .width(1)
                    .height(Fill)
                    .style(border_fill),
            )
            .width(grab)
            .height(Fill)
            .center_x(grab),
            iced::mouse::Interaction::ResizingHorizontally,
        )
    };
    mouse_area(zone)
        .on_press(Message::DockDragStart(area))
        .interaction(cursor)
        .into()
}

fn strip_icon(icon: icons::Icon, message: Message) -> Element<'static, Message> {
    button(icons::icon(icon, 12.0, theme::palette().text_dock))
        .padding(4)
        .style(icon_button)
        .on_press(message)
        .into()
}

impl App {
    /// WindTerm "Sender": compose text and send it with the same
    /// `CommandSend` path (and safety gates) as the Commands dialog.
    pub(super) fn sender_panel(&self) -> Element<'_, Message> {
        let header = row![
            action("Send explicitly", Message::CommandSend).style(primary),
            if self.tools.sender_sync {
                text("SYNC TARGETS · sends to every selected pane in this tab")
                    .size(12)
                    .color(danger())
            } else {
                text("COMMAND SENDER · FOCUSED PANE")
                    .size(12)
                    .color(muted())
            },
            space::horizontal(),
            button(text("Command palette").size(12))
                .padding([2, 6])
                .style(icon_button)
                .on_press(Message::Commands),
        ]
        .spacing(8)
        .height(28)
        .align_y(iced::Center);
        let editor = text_editor(&self.command_content)
            .placeholder("Remote-shell text to stage")
            .on_action(Message::CommandEdit)
            .font(iced::Font::MONOSPACE)
            .size(13)
            .height(Fill)
            .padding(8)
            .style(|theme: &Theme, status| {
                let palette = theme::palette();
                let mut style = text_editor::default(theme, status);
                style.background = iced::Background::Color(palette.list);
                style.border = iced::Border {
                    width: 1.0,
                    color: palette.border,
                    radius: 0.0.into(),
                };
                style
            });
        column![header, editor]
            .spacing(4)
            .padding(4)
            .width(Fill)
            .height(Fill)
            .into()
    }

    /// Body of a dock panel. Add new panel bodies here as they move into docks.
    pub(super) fn dock_panel_body(&self, panel: dock::Panel) -> Element<'_, Message> {
        match panel {
            Panel::Session => self.sidebar(),
            Panel::Filer => self.filer_panel(),
            Panel::Explorer => self.explorer_panel(),
            Panel::Transfer => self.transfer_panel(),
            Panel::Sender => self.sender_panel(),
            Panel::Outline | Panel::Shell => column![].into(),
        }
    }

    fn dock_strip(&self, area: Area) -> Element<'_, Message> {
        let state = self.docks.area(area);
        let p = theme::palette();
        let active = self.docks.active_panel(area);
        let mut strip = row![].spacing(4).align_y(iced::Center).height(Fill);
        if state.panels.len() == 1 {
            let panel = state.panels[0];
            strip = strip.push(
                row![
                    icons::marker(panel.marker(), 11.0),
                    text(panel.title())
                        .size(theme::metrics::UI_TEXT)
                        .color(p.text_dock)
                        .wrapping(iced::widget::text::Wrapping::None),
                ]
                .spacing(6)
                .align_y(iced::Center),
            );
        } else {
            for &panel in &state.panels {
                let is_active = Some(panel) == active;
                let label = container(
                    row![
                        icons::marker(panel.marker(), 11.0),
                        text(panel.title())
                            .size(theme::metrics::UI_TEXT)
                            .color(p.text_dock)
                            .wrapping(iced::widget::text::Wrapping::None),
                    ]
                    .spacing(6)
                    .align_y(iced::Center),
                )
                .padding([2, 8])
                .style(if is_active {
                    tab_active_fill
                } else {
                    tab_inactive_fill
                });
                // Active dock tab: lighter fill, 2px accent top line (as the
                // session tabs) and no bottom border so it merges with the body.
                let label: Element<'_, Message> = if is_active {
                    stack![
                        label,
                        container(space::horizontal())
                            .width(Fill)
                            .height(2)
                            .style(accent_fill)
                    ]
                    .into()
                } else {
                    label.into()
                };
                let tab = button(label)
                    .padding(if is_active {
                        iced::Padding {
                            top: 1.0,
                            right: 1.0,
                            bottom: 0.0,
                            left: 1.0,
                        }
                    } else {
                        iced::Padding::new(1.0)
                    })
                    .style(|_: &Theme, _| button::Style {
                        background: Some(theme::palette().border.into()),
                        ..Default::default()
                    })
                    .on_press(Message::DockActivate(panel));
                strip = strip.push(tab);
            }
        }
        strip = strip.push(space::horizontal());
        if area == Area::Bottom {
            let chevron = if state.collapsed {
                icons::Icon::ChevronUp
            } else {
                icons::Icon::ChevronDown
            };
            strip = strip.push(strip_icon(chevron, Message::DockCollapse(area)));
        }
        strip = strip.push(strip_icon(icons::Icon::Gear, Message::DockMenu(area)));
        if let Some(panel) = active {
            strip = strip.push(strip_icon(icons::Icon::Close, Message::DockClose(panel)));
        }
        container(strip)
            .padding([0, 6])
            .width(Fill)
            .height(theme::metrics::STRIP_HEIGHT)
            .clip(true)
            .align_y(iced::Center)
            .style(|_| fill_style(theme::palette().chrome))
            .into()
    }

    fn dock_area(&self, area: Area) -> Option<Element<'_, Message>> {
        let state = self.docks.area(area);
        if state.panels.is_empty() {
            return None;
        }
        let mut col = column![self.dock_strip(area)].width(Fill);
        if !(area == Area::Bottom && state.collapsed) {
            let body = self
                .docks
                .active_panel(area)
                .map(|panel| self.dock_panel_body(panel))
                .unwrap_or_else(|| column![].into());
            col = col.push(hline()).push(
                container(body)
                    .width(Fill)
                    .height(Fill)
                    .style(|_| fill_style(theme::palette().dock)),
            );
        }
        Some(col.into())
    }

    /// Main region between the menu bar and status bar.
    pub(super) fn dock_body(&self) -> Element<'_, Message> {
        let mut middle = row![].height(Fill);
        if let Some(left) = self.dock_area(Area::Left) {
            middle = middle
                .push(container(left).width(self.docks.left_width).height(Fill))
                .push(splitter(Area::Left));
        }
        middle = middle.push(container(self.workspace()).width(Fill).height(Fill));
        if let Some(right) = self.dock_area(Area::Right) {
            middle = middle
                .push(splitter(Area::Right))
                .push(container(right).width(self.docks.right_width).height(Fill));
        }
        let Some(bottom) = self.dock_area(Area::Bottom) else {
            return middle.into();
        };
        let (a, b) = split_portions(self.docks.bottom_ratio);
        if self.docks.bottom.collapsed {
            column![middle.height(Fill), hline(), bottom]
                .height(Fill)
                .into()
        } else {
            column![
                middle.height(FillPortion(a)),
                splitter(Area::Bottom),
                container(bottom).width(Fill).height(FillPortion(b)),
            ]
            .height(Fill)
            .into()
        }
    }

    fn dock_gear_menu(&self, area: Area) -> Element<'_, Message> {
        let mut rows = column![].width(GEAR_MENU_WIDTH);
        if let Some(panel) = self.docks.active_panel(area) {
            for (target, label) in [
                (Area::Left, "Move to Left"),
                (Area::Right, "Move to Right"),
                (Area::Bottom, "Move to Bottom"),
            ] {
                if target == area {
                    continue;
                }
                rows = rows.push(gear_row(label, Message::DockMove(panel, target)));
            }
            rows = rows.push(gear_row("Close", Message::DockClose(panel)));
        }
        opaque(container(rows).padding(1).style(|_| container::Style {
            background: Some(theme::palette().list.into()),
            border: Border {
                color: theme::palette().border,
                width: 1.0,
                radius: 0.0.into(),
            },
            ..Default::default()
        }))
    }

    pub(super) fn dock_menu_overlay<'a>(
        &'a self,
        body: Element<'a, Message>,
        area: Area,
    ) -> Element<'a, Message> {
        let top = theme::metrics::MENU_BAR_HEIGHT + theme::metrics::STRIP_HEIGHT;
        let menu = self.dock_gear_menu(area);
        let anchored: Element<'_, Message> = match area {
            Area::Left => container(menu)
                .padding(iced::Padding {
                    top,
                    left: (self.docks.left_width - GEAR_MENU_WIDTH).max(0.0),
                    ..iced::Padding::ZERO
                })
                .width(Fill)
                .height(Fill)
                .into(),
            Area::Right => container(menu)
                .padding(iced::Padding {
                    top,
                    ..iced::Padding::ZERO
                })
                .width(Fill)
                .height(Fill)
                .align_x(iced::Right)
                .into(),
            Area::Bottom => {
                let (a, _) = split_portions(self.docks.bottom_ratio);
                let above: Element<'_, Message> = if self.docks.bottom.collapsed {
                    space::vertical().height(Fill).into()
                } else {
                    space::vertical().height(FillPortion(a)).into()
                };
                let (_, b) = split_portions(self.docks.bottom_ratio);
                let below: Element<'_, Message> = if self.docks.bottom.collapsed {
                    space::vertical()
                        .height(theme::metrics::STATUS_BAR_HEIGHT)
                        .into()
                } else {
                    space::vertical().height(FillPortion(b)).into()
                };
                column![
                    space::vertical().height(theme::metrics::MENU_BAR_HEIGHT),
                    above,
                    space::vertical().height(theme::metrics::STRIP_HEIGHT + dock::SPLITTER_GRAB),
                    container(menu).width(Fill).align_x(iced::Right),
                    below,
                ]
                .width(Fill)
                .height(Fill)
                .into()
            }
        };
        stack![
            body,
            mouse_area(container(space::horizontal()).width(Fill).height(Fill))
                .on_press(Message::DockMenu(area)),
            anchored,
        ]
        .into()
    }
}

fn gear_row(label: &'static str, message: Message) -> Element<'static, Message> {
    button(
        text(label)
            .size(theme::metrics::UI_TEXT)
            .color(theme::palette().text),
    )
    .width(Fill)
    .height(24)
    .padding([3, 10])
    .style(menu_item)
    .on_press(message)
    .into()
}
