//! Vector toolbar icons. Drawn with egui's painter so they never depend on
//! a particular font having the right glyph (the bundled fonts render many
//! of these as empty squares).

use egui::{Color32, Pos2, Rect, Response, Sense, Shape, Stroke, Ui, Vec2};

#[derive(Clone, Copy)]
pub enum Glyph {
    Back,
    Forward,
    Up,
    Refresh,
    DualVertical,
    DualHorizontal,
    Single,
    Gear,
    Terminal,
    Copy,
    Move,
    Close,
    NewFolder,
    NewFile,
    Star,
    StarFilled,
    Cut,
    Paste,
    Link,
    Rename,
    Trash,
    Eye,
    Search,
    Compare,
}

pub fn button(ui: &mut Ui, glyph: Glyph, selected: bool, tip: &str) -> Response {
    button_enabled(ui, glyph, selected, true, tip)
}

/// Icon button that greys out and stops responding when `enabled` is false, so
/// actions that need a selection (or a filled clipboard) read as unavailable
/// instead of silently doing nothing.
pub fn button_enabled(
    ui: &mut Ui,
    glyph: Glyph,
    selected: bool,
    enabled: bool,
    tip: &str,
) -> Response {
    let size = Vec2::new(22.0, 20.0);
    if !enabled {
        let (rect, resp) = ui.allocate_exact_size(size, Sense::hover());
        let color = ui.visuals().widgets.noninteractive.fg_stroke.color.gamma_multiply(0.4);
        paint_glyph(ui.painter(), rect.shrink(4.0), glyph, color);
        return resp.on_hover_cursor(egui::CursorIcon::Default).on_hover_text(tip);
    }
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    let visuals = ui.style().interact(&resp);
    let fill = if selected {
        visuals.bg_fill
    } else {
        Color32::TRANSPARENT
    };
    ui.painter().rect(
        rect,
        visuals.corner_radius,
        fill,
        visuals.bg_stroke,
        egui::StrokeKind::Inside,
    );
    let color = if selected {
        visuals.fg_stroke.color
    } else {
        visuals.text_color()
    };
    paint_glyph(ui.painter(), rect.shrink(4.0), glyph, color);
    resp.on_hover_cursor(egui::CursorIcon::Default).on_hover_text(tip)
}

pub fn paint_glyph(p: &egui::Painter, r: Rect, glyph: Glyph, color: Color32) {
    // Scale the stroke with the icon so a 7pt glyph does not look twice as
    // heavy as a 14pt one drawn from the same primitives.
    let weight = (r.width().min(r.height()) * 0.105).clamp(1.1, 1.7);
    let stroke = Stroke::new(weight, color);
    match glyph {
        Glyph::Back => {
            let c = r.center();
            p.add(Shape::line(
                vec![
                    Pos2::new(c.x + 3.5, r.top()),
                    Pos2::new(r.left() + 1.0, c.y),
                    Pos2::new(c.x + 3.5, r.bottom()),
                ],
                stroke,
            ));
        }
        Glyph::Forward => {
            let c = r.center();
            p.add(Shape::line(
                vec![
                    Pos2::new(c.x - 3.5, r.top()),
                    Pos2::new(r.right() - 1.0, c.y),
                    Pos2::new(c.x - 3.5, r.bottom()),
                ],
                stroke,
            ));
        }
        Glyph::Up => {
            let c = r.center();
            p.add(Shape::line(
                vec![
                    Pos2::new(r.left(), c.y + 3.5),
                    Pos2::new(c.x, r.top() + 1.0),
                    Pos2::new(r.right(), c.y + 3.5),
                ],
                stroke,
            ));
        }
        Glyph::Refresh => {
            let c = r.center();
            let rad = r.width().min(r.height()) * 0.42;
            p.circle_stroke(c, rad, stroke);
            // Break the circle with a small gap at the top-right and an arrow head.
            p.line_segment(
                [Pos2::new(c.x + rad * 0.2, c.y - rad), Pos2::new(c.x + rad, c.y - rad * 0.15)],
                stroke,
            );
        }
        Glyph::DualVertical => {
            let gap = 2.0;
            let w = (r.width() - gap) / 2.0;
            p.rect_stroke(
                Rect::from_min_size(r.min, Vec2::new(w, r.height())),
                1.0,
                stroke,
                egui::StrokeKind::Inside,
            );
            p.rect_stroke(
                Rect::from_min_size(Pos2::new(r.min.x + w + gap, r.min.y), Vec2::new(w, r.height())),
                1.0,
                stroke,
                egui::StrokeKind::Inside,
            );
        }
        Glyph::DualHorizontal => {
            let gap = 2.0;
            let h = (r.height() - gap) / 2.0;
            p.rect_stroke(
                Rect::from_min_size(r.min, Vec2::new(r.width(), h)),
                1.0,
                stroke,
                egui::StrokeKind::Inside,
            );
            p.rect_stroke(
                Rect::from_min_size(Pos2::new(r.min.x, r.min.y + h + gap), Vec2::new(r.width(), h)),
                1.0,
                stroke,
                egui::StrokeKind::Inside,
            );
        }
        Glyph::Single => {
            p.rect_stroke(r, 1.0, stroke, egui::StrokeKind::Inside);
        }
        Glyph::Gear => {
            p.circle_stroke(r.center(), r.width() * 0.22, stroke);
            p.circle_stroke(r.center(), r.width() * 0.42, stroke);
        }
        Glyph::Terminal => {
            p.rect_stroke(r, 1.5, stroke, egui::StrokeKind::Inside);
            let y = r.center().y;
            p.line_segment(
                [Pos2::new(r.left() + 3.0, y - 2.5), Pos2::new(r.left() + 6.5, y)],
                stroke,
            );
            p.line_segment(
                [Pos2::new(r.left() + 6.5, y), Pos2::new(r.left() + 3.0, y + 2.5)],
                stroke,
            );
            p.line_segment(
                [Pos2::new(r.left() + 8.5, y + 2.5), Pos2::new(r.right() - 3.0, y + 2.5)],
                stroke,
            );
        }
        Glyph::Copy => {
            let back = Rect::from_min_max(
                Pos2::new(r.left() + 1.0, r.top() + 1.0),
                Pos2::new(r.right() - 4.0, r.bottom() - 4.0),
            );
            let front = Rect::from_min_max(
                Pos2::new(r.left() + 4.0, r.top() + 4.0),
                Pos2::new(r.right() - 1.0, r.bottom() - 1.0),
            );
            p.rect_stroke(back, 1.0, stroke, egui::StrokeKind::Inside);
            p.rect_filled(front, 1.0, Color32::from_black_alpha(180));
            p.rect_stroke(front, 1.0, stroke, egui::StrokeKind::Inside);
            let plus = Pos2::new(front.right() - 1.0, front.bottom() - 1.0);
            p.line_segment(
                [Pos2::new(plus.x - 5.0, plus.y), Pos2::new(plus.x + 1.0, plus.y)],
                Stroke::new(1.6, color),
            );
            p.line_segment(
                [Pos2::new(plus.x - 2.0, plus.y - 3.0), Pos2::new(plus.x - 2.0, plus.y + 3.0)],
                Stroke::new(1.6, color),
            );
        }
        Glyph::Move => {
            let doc = Rect::from_min_max(
                Pos2::new(r.left() + 0.5, r.top() + 2.0),
                Pos2::new(r.center().x + 1.0, r.bottom() - 1.0),
            );
            p.rect_stroke(doc, 1.0, stroke, egui::StrokeKind::Inside);
            let tip = Pos2::new(r.right() - 0.5, r.center().y - 1.0);
            let tail = Pos2::new(doc.right() + 1.0, r.center().y - 1.0);
            p.line_segment([tail, tip], Stroke::new(1.6, color));
            p.add(Shape::line(
                vec![
                    Pos2::new(tip.x - 4.0, tip.y - 3.5),
                    tip,
                    Pos2::new(tip.x - 4.0, tip.y + 3.5),
                ],
                Stroke::new(1.6, color),
            ));
        }
        Glyph::Close => {
            let r = r.shrink(3.5);
            p.line_segment([r.left_top(), r.right_bottom()], stroke);
            p.line_segment([r.right_top(), r.left_bottom()], stroke);
        }
        Glyph::NewFolder => {
            paint_folder(p, r, stroke);
            plus(p, Pos2::new(r.right() - 1.5, r.bottom() - 1.5), 2.6, color);
        }
        Glyph::NewFile => {
            paint_page(p, r, stroke);
            plus(p, Pos2::new(r.right() - 1.5, r.bottom() - 1.5), 2.6, color);
        }
        Glyph::Star | Glyph::StarFilled => {
            let c = r.center();
            let outer = r.width().min(r.height()) * 0.46;
            let mut pts = Vec::with_capacity(10);
            for i in 0..10 {
                // Start at the top point, alternating outer and inner radius.
                let ang = -std::f32::consts::FRAC_PI_2 + i as f32 * std::f32::consts::PI / 5.0;
                let rad = if i % 2 == 0 { outer } else { outer * 0.44 };
                pts.push(Pos2::new(c.x + rad * ang.cos(), c.y + rad * ang.sin()));
            }
            if matches!(glyph, Glyph::StarFilled) {
                p.add(Shape::convex_polygon(pts, color, Stroke::NONE));
            } else {
                pts.push(pts[0]);
                p.add(Shape::line(pts, stroke));
            }
        }
        Glyph::Cut => {
            // Two crossing blades over a pair of finger rings.
            let top = r.top() + 1.0;
            let cross = Pos2::new(r.center().x, r.bottom() - r.height() * 0.34);
            p.line_segment([Pos2::new(r.left() + 2.0, top), cross], stroke);
            p.line_segment([Pos2::new(r.right() - 2.0, top), cross], stroke);
            let ring = r.height() * 0.15;
            p.circle_stroke(Pos2::new(r.left() + 2.6, r.bottom() - ring), ring, stroke);
            p.circle_stroke(Pos2::new(r.right() - 2.6, r.bottom() - ring), ring, stroke);
        }
        Glyph::Paste => {
            let board = Rect::from_min_max(
                Pos2::new(r.left() + 1.0, r.top() + 2.0),
                Pos2::new(r.right() - 1.0, r.bottom() - 0.5),
            );
            p.rect_stroke(board, 1.5, stroke, egui::StrokeKind::Inside);
            // The clip at the top of the board.
            let clip = Rect::from_min_max(
                Pos2::new(board.center().x - 2.6, r.top() - 0.2),
                Pos2::new(board.center().x + 2.6, r.top() + 3.4),
            );
            p.rect_stroke(clip, 1.0, stroke, egui::StrokeKind::Inside);
        }
        Glyph::Link => {
            // Two interlocking rounded links, drawn on a diagonal.
            let h = r.height() * 0.34;
            let w = r.width() * 0.52;
            let a = Rect::from_center_size(
                Pos2::new(r.center().x - w * 0.26, r.center().y - h * 0.42),
                egui::vec2(w, h),
            );
            let b = Rect::from_center_size(
                Pos2::new(r.center().x + w * 0.26, r.center().y + h * 0.42),
                egui::vec2(w, h),
            );
            p.rect_stroke(a, h * 0.5, stroke, egui::StrokeKind::Inside);
            p.rect_stroke(b, h * 0.5, stroke, egui::StrokeKind::Inside);
        }
        Glyph::Rename => {
            // Pencil on a diagonal, with a nib at the lower left.
            let tip = Pos2::new(r.left() + 1.2, r.bottom() - 1.2);
            let tail = Pos2::new(r.right() - 2.2, r.top() + 2.2);
            p.line_segment([tip, tail], stroke);
            let across = r.width() * 0.22;
            p.line_segment(
                [
                    Pos2::new(tail.x - across, tail.y),
                    Pos2::new(tail.x, tail.y + across),
                ],
                stroke,
            );
            p.line_segment(
                [tip, Pos2::new(tip.x + across * 0.9, tip.y - across * 0.1)],
                stroke,
            );
        }
        Glyph::Trash => {
            let lid_y = r.top() + r.height() * 0.24;
            p.line_segment(
                [Pos2::new(r.left() + 1.0, lid_y), Pos2::new(r.right() - 1.0, lid_y)],
                stroke,
            );
            // Handle above the lid.
            p.line_segment(
                [
                    Pos2::new(r.center().x - 2.0, lid_y - 1.8),
                    Pos2::new(r.center().x + 2.0, lid_y - 1.8),
                ],
                stroke,
            );
            // Tapered can.
            let bl = Pos2::new(r.left() + 2.4, r.bottom() - 0.8);
            let br = Pos2::new(r.right() - 2.4, r.bottom() - 0.8);
            p.line_segment([Pos2::new(r.left() + 1.8, lid_y), bl], stroke);
            p.line_segment([Pos2::new(r.right() - 1.8, lid_y), br], stroke);
            p.line_segment([bl, br], stroke);
        }
        Glyph::Eye => {
            let c = r.center();
            let w = r.width() * 0.44;
            let h = r.height() * 0.26;
            // Lens as two arcs approximated by polylines.
            let arc = |sign: f32| {
                let mut pts = Vec::with_capacity(9);
                for i in 0..=8 {
                    let t = -1.0 + i as f32 * 0.25;
                    pts.push(Pos2::new(c.x + t * w, c.y + sign * h * (1.0 - t * t)));
                }
                Shape::line(pts, stroke)
            };
            p.add(arc(1.0));
            p.add(arc(-1.0));
            p.circle_stroke(c, h * 0.62, stroke);
        }
        Glyph::Search => {
            let rad = r.width().min(r.height()) * 0.30;
            let c = Pos2::new(r.left() + rad + 1.2, r.top() + rad + 1.2);
            p.circle_stroke(c, rad, stroke);
            p.line_segment(
                [
                    Pos2::new(c.x + rad * 0.72, c.y + rad * 0.72),
                    Pos2::new(r.right() - 1.0, r.bottom() - 1.0),
                ],
                Stroke::new(weight * 1.15, color),
            );
        }
        Glyph::Compare => {
            // Two panes with a divider: the folder-compare view.
            let body = r.shrink(1.2);
            p.rect_stroke(body, 1.5, stroke, egui::StrokeKind::Inside);
            p.line_segment(
                [
                    Pos2::new(body.center().x, body.top()),
                    Pos2::new(body.center().x, body.bottom()),
                ],
                stroke,
            );
        }
    }
}

/// Shared folder outline: a tab on the upper left, then the body.
fn paint_folder(p: &egui::Painter, r: Rect, stroke: Stroke) {
    let body = Rect::from_min_max(
        Pos2::new(r.left() + 0.8, r.top() + 3.0),
        Pos2::new(r.right() - 0.8, r.bottom() - 1.4),
    );
    p.rect_stroke(body, 1.5, stroke, egui::StrokeKind::Inside);
    p.add(Shape::line(
        vec![
            Pos2::new(body.left(), body.top()),
            Pos2::new(body.left() + body.width() * 0.16, r.top() + 1.0),
            Pos2::new(body.left() + body.width() * 0.52, r.top() + 1.0),
            Pos2::new(body.left() + body.width() * 0.62, body.top()),
        ],
        stroke,
    ));
}

/// Shared page outline with a folded corner.
fn paint_page(p: &egui::Painter, r: Rect, stroke: Stroke) {
    let fold = r.width() * 0.3;
    let left = r.left() + 1.8;
    let right = r.right() - 1.8;
    let top = r.top() + 1.0;
    let bottom = r.bottom() - 1.2;
    p.add(Shape::line(
        vec![
            Pos2::new(left, top),
            Pos2::new(right - fold, top),
            Pos2::new(right, top + fold),
            Pos2::new(right, bottom),
            Pos2::new(left, bottom),
            Pos2::new(left, top),
        ],
        stroke,
    ));
    p.add(Shape::line(
        vec![
            Pos2::new(right - fold, top),
            Pos2::new(right - fold, top + fold),
            Pos2::new(right, top + fold),
        ],
        stroke,
    ));
}

/// Small filled plus badge, for the "new ..." variants.
fn plus(p: &egui::Painter, center: Pos2, size: f32, color: Color32) {
    let s = Stroke::new((size * 0.62).max(1.2), color);
    p.line_segment(
        [
            Pos2::new(center.x - size, center.y),
            Pos2::new(center.x + size, center.y),
        ],
        s,
    );
    p.line_segment(
        [
            Pos2::new(center.x, center.y - size),
            Pos2::new(center.x, center.y + size),
        ],
        s,
    );
}
