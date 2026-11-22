#![allow(dead_code)] // wired in by later UI stages
//! Original, canvas-drawn flat line icons.
//!
//! Every icon is described as a list of simple geometric [`Shape`]s inside a
//! 16x16 design box, so the geometry is testable without a renderer and
//! renders identically on every platform (no icon fonts / unicode glyphs).

use iced::widget::canvas::{self, Frame, Path, Stroke, path::Arc, stroke::LineCap};
use iced::{Color, Element, Length, Point, Radians, Rectangle, Renderer, Size, Theme, mouse};

/// Side of the square design box all shapes are expressed in.
pub(super) const DESIGN: f32 = 16.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) enum Icon {
    Close,
    Plus,
    Minus,
    Menu,
    MoreVertical,
    Gear,
    ChevronUp,
    ChevronDown,
    ChevronRight,
    TriangleRight,
    TriangleDown,
    TriangleUp,
    ArrowUp,
    Refresh,
    ClearPath,
    Info,
    Folder,
    File,
    SplitRight,
    SplitDown,
    Terminal,
    Play,
    Stop,
    Download,
    Upload,
    Edit,
    Trash,
    Lock,
    Search,
    Circle,
    CircleHollow,
    Square,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) enum Shape {
    Line {
        from: (f32, f32),
        to: (f32, f32),
    },
    Polyline {
        points: Vec<(f32, f32)>,
        closed: bool,
        filled: bool,
    },
    Circle {
        center: (f32, f32),
        radius: f32,
        filled: bool,
    },
    Rect {
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        filled: bool,
    },
    Arc {
        center: (f32, f32),
        radius: f32,
        start_deg: f32,
        end_deg: f32,
    },
}

fn line(from: (f32, f32), to: (f32, f32)) -> Shape {
    Shape::Line { from, to }
}

fn open(points: &[(f32, f32)]) -> Shape {
    Shape::Polyline {
        points: points.to_vec(),
        closed: false,
        filled: false,
    }
}

fn closed(points: &[(f32, f32)]) -> Shape {
    Shape::Polyline {
        points: points.to_vec(),
        closed: true,
        filled: false,
    }
}

fn filled(points: &[(f32, f32)]) -> Shape {
    Shape::Polyline {
        points: points.to_vec(),
        closed: true,
        filled: true,
    }
}

fn circle(center: (f32, f32), radius: f32, filled: bool) -> Shape {
    Shape::Circle {
        center,
        radius,
        filled,
    }
}

fn rect(x: f32, y: f32, w: f32, h: f32, filled: bool) -> Shape {
    Shape::Rect { x, y, w, h, filled }
}

fn arc(center: (f32, f32), radius: f32, start_deg: f32, end_deg: f32) -> Shape {
    Shape::Arc {
        center,
        radius,
        start_deg,
        end_deg,
    }
}

impl Icon {
    pub(super) const ALL: [Icon; 32] = [
        Icon::Close,
        Icon::Plus,
        Icon::Minus,
        Icon::Menu,
        Icon::MoreVertical,
        Icon::Gear,
        Icon::ChevronUp,
        Icon::ChevronDown,
        Icon::ChevronRight,
        Icon::TriangleRight,
        Icon::TriangleDown,
        Icon::TriangleUp,
        Icon::ArrowUp,
        Icon::Refresh,
        Icon::ClearPath,
        Icon::Info,
        Icon::Folder,
        Icon::File,
        Icon::SplitRight,
        Icon::SplitDown,
        Icon::Terminal,
        Icon::Play,
        Icon::Stop,
        Icon::Download,
        Icon::Upload,
        Icon::Edit,
        Icon::Trash,
        Icon::Lock,
        Icon::Search,
        Icon::Circle,
        Icon::CircleHollow,
        Icon::Square,
    ];

    /// Geometry in the 16x16 design box (angles in degrees, clockwise from +x
    /// because the y axis points down).
    pub(super) fn shapes(self) -> Vec<Shape> {
        match self {
            Icon::Close => vec![
                line((4.0, 4.0), (12.0, 12.0)),
                line((12.0, 4.0), (4.0, 12.0)),
            ],
            Icon::Plus => vec![line((8.0, 3.0), (8.0, 13.0)), line((3.0, 8.0), (13.0, 8.0))],
            Icon::Minus => vec![line((3.0, 8.0), (13.0, 8.0))],
            Icon::Menu => vec![
                line((2.5, 4.5), (13.5, 4.5)),
                line((2.5, 8.0), (13.5, 8.0)),
                line((2.5, 11.5), (13.5, 11.5)),
            ],
            Icon::MoreVertical => vec![
                circle((8.0, 3.5), 1.25, true),
                circle((8.0, 8.0), 1.25, true),
                circle((8.0, 12.5), 1.25, true),
            ],
            Icon::Gear => {
                let mut shapes = vec![
                    circle((8.0, 8.0), 4.0, false),
                    circle((8.0, 8.0), 1.5, false),
                ];
                for i in 0..8 {
                    let a = (i as f32) * std::f32::consts::FRAC_PI_4;
                    let (s, c) = a.sin_cos();
                    shapes.push(line(
                        (8.0 + 4.0 * c, 8.0 + 4.0 * s),
                        (8.0 + 6.0 * c, 8.0 + 6.0 * s),
                    ));
                }
                shapes
            }
            Icon::ChevronUp => vec![open(&[(4.0, 10.0), (8.0, 6.0), (12.0, 10.0)])],
            Icon::ChevronDown => vec![open(&[(4.0, 6.0), (8.0, 10.0), (12.0, 6.0)])],
            Icon::ChevronRight => vec![open(&[(6.0, 4.0), (10.0, 8.0), (6.0, 12.0)])],
            Icon::TriangleRight => vec![filled(&[(6.0, 4.5), (10.5, 8.0), (6.0, 11.5)])],
            Icon::TriangleDown => vec![filled(&[(4.5, 6.0), (11.5, 6.0), (8.0, 10.5)])],
            Icon::TriangleUp => vec![filled(&[(4.5, 10.0), (11.5, 10.0), (8.0, 5.5)])],
            Icon::ArrowUp => vec![
                line((8.0, 3.0), (8.0, 13.0)),
                open(&[(4.0, 7.0), (8.0, 3.0), (12.0, 7.0)]),
            ],
            Icon::Refresh => vec![
                arc((8.0, 8.0), 5.0, -60.0, 240.0),
                open(&[(10.5, 1.5), (10.5, 3.67), (12.67, 3.67)]),
            ],
            Icon::ClearPath => vec![
                closed(&[
                    (1.5, 8.0),
                    (5.0, 3.5),
                    (14.5, 3.5),
                    (14.5, 12.5),
                    (5.0, 12.5),
                ]),
                line((7.5, 6.0), (11.5, 10.0)),
                line((11.5, 6.0), (7.5, 10.0)),
            ],
            Icon::Info => vec![
                circle((8.0, 8.0), 6.0, false),
                line((8.0, 7.0), (8.0, 11.5)),
                circle((8.0, 4.75), 0.75, true),
            ],
            Icon::Folder => vec![filled(&[
                (1.5, 3.5),
                (6.0, 3.5),
                (7.5, 5.0),
                (14.5, 5.0),
                (14.5, 12.5),
                (1.5, 12.5),
            ])],
            Icon::File => vec![
                closed(&[
                    (3.5, 1.5),
                    (9.5, 1.5),
                    (12.5, 4.5),
                    (12.5, 14.5),
                    (3.5, 14.5),
                ]),
                open(&[(9.5, 1.5), (9.5, 4.5), (12.5, 4.5)]),
            ],
            Icon::SplitRight => vec![
                rect(2.5, 3.5, 11.0, 9.0, false),
                line((8.0, 3.5), (8.0, 12.5)),
            ],
            Icon::SplitDown => vec![
                rect(2.5, 3.5, 11.0, 9.0, false),
                line((2.5, 8.0), (13.5, 8.0)),
            ],
            Icon::Terminal => vec![
                rect(1.5, 2.5, 13.0, 11.0, false),
                open(&[(4.5, 6.0), (6.5, 8.0), (4.5, 10.0)]),
                line((8.0, 10.5), (11.5, 10.5)),
            ],
            Icon::Play => vec![filled(&[(4.5, 3.0), (12.5, 8.0), (4.5, 13.0)])],
            Icon::Stop => vec![rect(4.0, 4.0, 8.0, 8.0, true)],
            Icon::Download => vec![
                line((8.0, 2.0), (8.0, 10.0)),
                open(&[(4.5, 6.5), (8.0, 10.0), (11.5, 6.5)]),
                open(&[(2.5, 10.5), (2.5, 13.5), (13.5, 13.5), (13.5, 10.5)]),
            ],
            Icon::Upload => vec![
                line((8.0, 10.0), (8.0, 2.0)),
                open(&[(4.5, 5.5), (8.0, 2.0), (11.5, 5.5)]),
                open(&[(2.5, 10.5), (2.5, 13.5), (13.5, 13.5), (13.5, 10.5)]),
            ],
            Icon::Edit => vec![
                closed(&[
                    (2.5, 13.5),
                    (3.0, 10.5),
                    (10.5, 3.0),
                    (13.0, 5.5),
                    (5.5, 13.0),
                ]),
                line((9.0, 4.5), (11.5, 7.0)),
            ],
            Icon::Trash => vec![
                line((2.5, 4.5), (13.5, 4.5)),
                open(&[(6.0, 4.5), (6.5, 2.5), (9.5, 2.5), (10.0, 4.5)]),
                closed(&[(4.0, 4.5), (12.0, 4.5), (11.0, 14.0), (5.0, 14.0)]),
                line((7.0, 7.0), (7.0, 11.5)),
                line((9.0, 7.0), (9.0, 11.5)),
            ],
            Icon::Lock => vec![
                rect(3.5, 7.5, 9.0, 7.0, false),
                arc((8.0, 5.5), 2.5, 180.0, 360.0),
                line((5.5, 5.5), (5.5, 7.5)),
                line((10.5, 5.5), (10.5, 7.5)),
                line((8.0, 10.0), (8.0, 12.0)),
            ],
            Icon::Search => vec![
                circle((6.5, 6.5), 4.0, false),
                line((9.5, 9.5), (13.5, 13.5)),
            ],
            Icon::Circle => vec![circle((8.0, 8.0), 4.0, true)],
            Icon::CircleHollow => vec![circle((8.0, 8.0), 4.0, false)],
            Icon::Square => vec![rect(4.0, 4.0, 8.0, 8.0, true)],
        }
    }
}

/// Canvas program drawing one [`Icon`] scaled to the widget bounds.
#[derive(Debug, Clone, Copy)]
pub(super) struct IconProgram {
    pub(super) icon: Icon,
    pub(super) color: Color,
}

impl<Message> canvas::Program<Message> for IconProgram {
    type State = ();

    fn draw(
        &self,
        _state: &Self::State,
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<canvas::Geometry> {
        let mut frame = Frame::new(renderer, bounds.size());
        let side = bounds.width.min(bounds.height);
        let scale = side / DESIGN;
        let ox = (bounds.width - side) / 2.0;
        let oy = (bounds.height - side) / 2.0;
        let p = |(x, y): (f32, f32)| Point::new(ox + x * scale, oy + y * scale);
        let stroke = Stroke::default()
            .with_color(self.color)
            .with_width(1.25 * scale)
            .with_line_cap(LineCap::Round);

        for shape in self.icon.shapes() {
            match shape {
                Shape::Line { from, to } => {
                    frame.stroke(&Path::line(p(from), p(to)), stroke);
                }
                Shape::Polyline {
                    points,
                    closed,
                    filled,
                } => {
                    if points.is_empty() {
                        continue;
                    }
                    let path = Path::new(|b| {
                        b.move_to(p(points[0]));
                        for &pt in &points[1..] {
                            b.line_to(p(pt));
                        }
                        if closed || filled {
                            b.close();
                        }
                    });
                    if filled {
                        frame.fill(&path, self.color);
                    } else {
                        frame.stroke(&path, stroke);
                    }
                }
                Shape::Circle {
                    center,
                    radius,
                    filled,
                } => {
                    let path = Path::circle(p(center), radius * scale);
                    if filled {
                        frame.fill(&path, self.color);
                    } else {
                        frame.stroke(&path, stroke);
                    }
                }
                Shape::Rect { x, y, w, h, filled } => {
                    let path = Path::rectangle(p((x, y)), Size::new(w * scale, h * scale));
                    if filled {
                        frame.fill(&path, self.color);
                    } else {
                        frame.stroke(&path, stroke);
                    }
                }
                Shape::Arc {
                    center,
                    radius,
                    start_deg,
                    end_deg,
                } => {
                    let path = Path::new(|b| {
                        b.arc(Arc {
                            center: p(center),
                            radius: radius * scale,
                            start_angle: Radians(start_deg.to_radians()),
                            end_angle: Radians(end_deg.to_radians()),
                        });
                    });
                    frame.stroke(&path, stroke);
                }
            }
        }

        vec![frame.into_geometry()]
    }
}

/// Fixed-size (size x size) canvas element drawing `icon` in `color`.
pub(super) fn icon<'a, Message: 'a>(icon: Icon, size: f32, color: Color) -> Element<'a, Message> {
    canvas::Canvas::new(IconProgram { icon, color })
        .width(Length::Fixed(size))
        .height(Length::Fixed(size))
        .into()
}

/// Filled square marker (e.g. before dock titles).
pub(super) fn marker<'a, Message: 'a>(color: Color, size: f32) -> Element<'a, Message> {
    icon(Icon::Square, size, color)
}

/// Filled circular dot (e.g. status indicators).
pub(super) fn dot<'a, Message: 'a>(color: Color, size: f32) -> Element<'a, Message> {
    icon(Icon::Circle, size, color)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn inside(v: f32) -> bool {
        (0.0..=DESIGN).contains(&v)
    }

    fn assert_point(icon: Icon, (x, y): (f32, f32)) {
        assert!(
            inside(x) && inside(y),
            "{icon:?}: point ({x}, {y}) outside design box"
        );
    }

    #[test]
    fn every_icon_has_shapes() {
        for icon in Icon::ALL {
            assert!(!icon.shapes().is_empty(), "{icon:?} has no shapes");
        }
    }

    #[test]
    fn all_has_no_duplicates() {
        let set: HashSet<Icon> = Icon::ALL.iter().copied().collect();
        assert_eq!(set.len(), Icon::ALL.len());
    }

    #[test]
    fn coordinates_within_design_box() {
        for icon in Icon::ALL {
            for shape in icon.shapes() {
                match shape {
                    Shape::Line { from, to } => {
                        assert_point(icon, from);
                        assert_point(icon, to);
                    }
                    Shape::Polyline { points, .. } => {
                        assert!(points.len() >= 2, "{icon:?}: degenerate polyline");
                        for pt in points {
                            assert_point(icon, pt);
                        }
                    }
                    Shape::Circle { center, radius, .. } | Shape::Arc { center, radius, .. } => {
                        assert!(radius > 0.0, "{icon:?}: non-positive radius");
                        assert_point(icon, (center.0 - radius, center.1 - radius));
                        assert_point(icon, (center.0 + radius, center.1 + radius));
                    }
                    Shape::Rect { x, y, w, h, .. } => {
                        assert!(w > 0.0 && h > 0.0, "{icon:?}: empty rect");
                        assert_point(icon, (x, y));
                        assert_point(icon, (x + w, y + h));
                    }
                }
            }
        }
    }

    #[test]
    fn arcs_have_extent() {
        for icon in Icon::ALL {
            for shape in icon.shapes() {
                if let Shape::Arc {
                    start_deg, end_deg, ..
                } = shape
                {
                    assert!(start_deg != end_deg, "{icon:?}: zero-length arc");
                }
            }
        }
    }
}
