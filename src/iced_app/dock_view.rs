//! Dock area rendering (spec section 5): left/right/bottom areas with a
//! title or tab strip, gear menu and the active panel body.

use super::*;
use dock::{Area, Panel};
use iced::Length::FillPortion;
use iced::widget::{column, row};

const GEAR_MENU_WIDTH: f32 = 200.0;

/// Panels that have a body in this stage. Later stages add Filer, Shell, ...
pub(super) fn available_panels() -> Vec<dock::Panel> {
    vec![Panel::Session, Panel::Filer]
}

fn split_portions(ratio: f32) -> (u16, u16) {
    let r = ratio.clamp(0.0, 1.0);
    let b = ((r * 1000.0) as u16).max(1);
    let a = (((1.0 - r) * 1000.0) as u16).max(1);
    (a, b)
}

fn vline<'a>() -> Element<'a, Message> {
    container(space::horizontal())
        .width(1)
        .height(Fill)
        .style(border_fill)
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
    /// Body of a dock panel. Add new panel bodies here as they move into docks.
    pub(super) fn dock_panel_body(&self, panel: dock::Panel) -> Element<'_, Message> {
        match panel {
            Panel::Session => self.sidebar(),
            Panel::Filer if !self.tabs.is_empty() => self.files(),
            Panel::Filer => container(
                text("Open a session to browse files")
                    .size(13)
                    .color(muted()),
            )
            .center(Fill)
            .into(),
            Panel::Sender | Panel::Outline | Panel::Transfer | Panel::Explorer | Panel::Shell => {
                column![].into()
            }
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
                        .color(p.text_dock),
                ]
                .spacing(6)
                .align_y(iced::Center),
            );
        } else {
            for &panel in &state.panels {
                let is_active = Some(panel) == active;
                let tab = button(
                    container(
                        row![
                            icons::marker(panel.marker(), 11.0),
                            text(panel.title())
                                .size(theme::metrics::UI_TEXT)
                                .color(p.text_dock),
                        ]
                        .spacing(6)
                        .align_y(iced::Center),
                    )
                    .padding([2, 8])
                    .style(if is_active {
                        tab_active_fill
                    } else {
                        tab_inactive_fill
                    }),
                )
                .padding(1)
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
                .push(vline());
        }
        middle = middle.push(container(self.workspace()).width(Fill).height(Fill));
        if let Some(right) = self.dock_area(Area::Right) {
            middle = middle
                .push(vline())
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
                hline(),
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
                    space::vertical().height(theme::metrics::STRIP_HEIGHT + 1.0),
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
