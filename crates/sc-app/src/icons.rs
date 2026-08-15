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
}

pub fn button(ui: &mut Ui, glyph: Glyph, selected: bool, tip: &str) -> Response {
    let size = Vec2::new(22.0, 20.0);
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
    paint(ui, rect.shrink(4.0), glyph, color);
    resp.on_hover_cursor(egui::CursorIcon::Default).on_hover_text(tip)
}

fn paint(ui: &mut Ui, r: Rect, glyph: Glyph, color: Color32) {
    let stroke = Stroke::new(1.4, color);
    let p = ui.painter();
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
    }
}
