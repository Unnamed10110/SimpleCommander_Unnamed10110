//! Mouse interaction primitives for the file panes and the tab strip.
//!
//! Every hit test here goes through egui's own [`egui::Response`], which
//! already accounts for layer order and occlusion: a row covered by a popup
//! reports `contains_pointer() == false`, and only the top-most widget at a
//! position reports `true`. Nothing in this module — or its callers — decides
//! *what the pointer is over* by testing a stored `Rect` against a pointer
//! position. That pattern is what made the old hitboxes unreliable, because a
//! rect recorded during layout has no idea what was drawn over it afterwards.
//!
//! Two consequences worth knowing when editing the callers:
//!
//! * **Widget order is the drop-target priority.** Register the background
//!   interact *before* the rows, and rows automatically win their own area
//!   while the background wins only the gaps. No priority list is needed.
//! * **`dnd_release_payload` consumes the payload**, so exactly one target can
//!   accept a given drop. Ordering falls out of egui's hit test rather than
//!   from us picking a winner.

use egui::{Color32, CursorIcon, Rect, Ui};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// Tab-strip reorder drag.
#[derive(Clone)]
pub struct TabDrag {
    pub uid: u64,
}

/// In-app file drag: file rows onto a folder row, a tab header, or a pane body.
#[derive(Clone)]
pub struct FileDrag {
    pub paths: Vec<PathBuf>,
    /// Pane the drag started in, so a move can refresh the source listing.
    pub from_pane: usize,
}

/// What a drop will do. A plain drag copies; Ctrl or Shift at drop time moves.
///
/// The modifier is read at *drop* time, not at drag start, so the user can
/// change their mind mid-drag (and so Ctrl+click can still mean "toggle
/// selection" when the gesture begins).
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum DropEffect {
    Copy,
    Move,
}

impl DropEffect {
    pub fn current(ctx: &egui::Context) -> Self {
        let m = ctx.input(|i| i.modifiers);
        if m.ctrl || m.command || m.shift {
            Self::Move
        } else {
            Self::Copy
        }
    }

    pub fn is_move(self) -> bool {
        matches!(self, Self::Move)
    }

    pub fn verb(self) -> &'static str {
        match self {
            Self::Copy => "Copy",
            Self::Move => "Move",
        }
    }

    pub fn glyph(self) -> crate::icons::Glyph {
        match self {
            Self::Copy => crate::icons::Glyph::Copy,
            Self::Move => crate::icons::Glyph::Move,
        }
    }
}

/// What the pointer is currently over during a file drag, for the badge.
///
/// This is display-only. It is written by whichever drop target has the
/// pointer this frame and read by [`draw_drag_badge`] at the end of the frame,
/// so being one widget behind is harmless — unlike hit testing, which must
/// never rely on stored rects.
pub struct DropHint {
    /// Folder name to show ("Documents"), not the full path.
    pub dest: String,
    pub allowed: bool,
}

/// True when at least one source could actually land in `dest`.
///
/// Rejects sources already in `dest`, dropping a folder onto itself, and
/// dropping a folder into its own subtree.
pub fn drop_allowed(sources: &[PathBuf], dest: &Path) -> bool {
    sources
        .iter()
        .any(|p| p.parent() != Some(dest) && p.as_path() != dest && !dest.starts_with(p))
}

/// Short display name for a destination folder.
pub fn dest_label(dest: &Path) -> String {
    dest.file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| dest.to_string_lossy().into_owned())
}

/// The modifiers held when a file list was pressed, remembered until release.
///
/// Needed because a click resolves on *release*, by which time the user may
/// have let go of Ctrl or Shift; the gesture should still do what they started.
/// Whether the press becomes a click, a rubber-band, or a file drag is decided
/// by egui's own click-versus-drag resolution, never by raw pointer polling.
#[derive(Clone)]
pub struct ListPress {
    pub pane: usize,
    pub tab_uid: u64,
    pub ctrl: bool,
    pub shift: bool,
}

/// Where the file list's rows actually sit, measured from the rows egui laid
/// out this frame.
///
/// Nothing here is assumed. `egui_extras` advances rows by
/// `row_height + item_spacing.y` — not by `row_height` — and the first row's
/// screen position depends on the header height and the scroll offset. Deriving
/// both from real row rects keeps the rubber-band exact no matter how those
/// change, and avoids an error in the pitch that would otherwise accumulate
/// with drag distance.
#[derive(Clone, Copy)]
pub struct RowMetrics {
    /// Screen y that corresponds to content y 0 (the top of row 0), which moves
    /// as the list scrolls.
    pub origin_y: f32,
    /// Distance between the tops of consecutive rows.
    pub pitch: f32,
}

impl RowMetrics {
    /// Calibrate from `(view row, row top in screen space)` samples.
    ///
    /// Two distinct rows give the pitch directly; with fewer, fall back to the
    /// documented `row_height + item_spacing.y`.
    pub fn from_rows(samples: &[(usize, f32)], fallback_pitch: f32) -> Self {
        let fallback_pitch = if fallback_pitch > 0.5 {
            fallback_pitch
        } else {
            1.0
        };
        let first = samples.first().copied();
        let last = samples.last().copied();
        match (first, last) {
            (Some((a, top_a)), Some((b, top_b))) if b > a => {
                let measured = (top_b - top_a) / (b - a) as f32;
                let pitch = if measured > 0.5 { measured } else { fallback_pitch };
                Self {
                    origin_y: top_a - a as f32 * pitch,
                    pitch,
                }
            }
            (Some((a, top_a)), _) => Self {
                origin_y: top_a - a as f32 * fallback_pitch,
                pitch: fallback_pitch,
            },
            _ => Self {
                origin_y: 0.0,
                pitch: fallback_pitch,
            },
        }
    }

    pub fn to_content(self, screen_y: f32) -> f32 {
        screen_y - self.origin_y
    }

    pub fn to_screen(self, content_y: f32) -> f32 {
        content_y + self.origin_y
    }

    /// Total height of all rows, for clamping autoscroll.
    pub fn content_height(self, n: usize) -> f32 {
        n as f32 * self.pitch
    }
}

/// Rubber-band selection, anchored in *content* space so it survives scrolling.
///
/// Content space has its origin at the top of row 0, so scrolling during the
/// drag moves the visible band without moving the anchor.
#[derive(Clone)]
pub struct Marquee {
    pub pane: usize,
    pub tab_uid: u64,
    pub anchor_content_y: f32,
    pub anchor_x: f32,
    /// Ctrl was held at press: union with the selection that existed then.
    pub additive: bool,
    pub keep: HashSet<u32>,
}

impl Marquee {
    /// Inclusive view-row range the band covers.
    ///
    /// Any row the band touches is included, which is what Explorer does.
    /// Because this is arithmetic over `metrics`, rows scrolled out of view are
    /// covered exactly like visible ones — scanning drawn row rects (the
    /// original approach) could only ever see the virtualized window.
    pub fn rows(&self, cur_content_y: f32, metrics: RowMetrics, n: usize) -> Option<(usize, usize)> {
        if n == 0 || metrics.pitch <= 0.0 {
            return None;
        }
        let (top, bottom) = if self.anchor_content_y <= cur_content_y {
            (self.anchor_content_y, cur_content_y)
        } else {
            (cur_content_y, self.anchor_content_y)
        };
        if bottom < 0.0 || top >= metrics.content_height(n) {
            return None;
        }
        let first = (top.max(0.0) / metrics.pitch).floor() as usize;
        if first >= n {
            return None;
        }
        let last = ((bottom.max(0.0) / metrics.pitch).floor() as usize).min(n - 1);
        Some((first, last))
    }
}

/// Outline a rect as a drop target and set the matching cursor.
///
/// Takes a painter rather than a `Ui` so it can be called from inside a table
/// row closure, where the parent `Ui` is already borrowed by the table.
pub fn paint_drop_target(
    painter: &egui::Painter,
    ctx: &egui::Context,
    rect: Rect,
    theme: &crate::theme::Theme,
    allowed: bool,
) {
    let color = if allowed { theme.accent } else { theme.error };
    if allowed {
        painter.rect_filled(rect, 4.0, color.gamma_multiply(0.10));
    }
    painter.rect_stroke(
        rect,
        4.0,
        egui::Stroke::new(2.0, color),
        egui::StrokeKind::Inside,
    );
    ctx.set_cursor_icon(if allowed {
        match DropEffect::current(ctx) {
            DropEffect::Copy => CursorIcon::Copy,
            DropEffect::Move => CursorIcon::Move,
        }
    } else {
        CursorIcon::NotAllowed
    });
}

/// Rubber-band rectangle.
pub fn paint_marquee(ui: &Ui, rect: Rect, accent: Color32) {
    let p = ui.painter();
    p.rect_filled(rect, 0.0, accent.gamma_multiply(0.15));
    p.rect_stroke(
        rect,
        0.0,
        egui::Stroke::new(1.0, accent),
        egui::StrokeKind::Inside,
    );
}

/// Vertical insertion caret for tab reordering.
pub fn paint_insert_caret(ui: &Ui, x: f32, y: egui::Rangef, accent: Color32) {
    let p = ui.painter();
    p.vline(x, y, egui::Stroke::new(2.5, accent));
    // Small nub at the top so the caret reads as an insertion point rather
    // than a tab border.
    p.circle_filled(egui::pos2(x, y.min + 1.5), 2.5, accent);
}

/// Badge that follows the pointer during an in-app file drag, naming the
/// destination and whether the drop copies or moves.
pub fn draw_drag_badge(
    ctx: &egui::Context,
    theme: &crate::theme::Theme,
    hint: Option<&DropHint>,
) {
    if !egui::DragAndDrop::has_payload_of_type::<FileDrag>(ctx) {
        return;
    }
    // Keep animating: the badge tracks the cursor and the effect flips live
    // as Ctrl/Shift are pressed.
    ctx.request_repaint();
    let Some(pos) = ctx.pointer_hover_pos().or_else(|| ctx.pointer_interact_pos()) else {
        return;
    };
    let count = egui::DragAndDrop::payload::<FileDrag>(ctx)
        .map(|d| d.paths.len())
        .unwrap_or(1);
    let effect = DropEffect::current(ctx);
    let noun = if count == 1 {
        "1 item".to_string()
    } else {
        format!("{count} items")
    };
    let allowed = hint.map(|h| h.allowed).unwrap_or(true);
    let accent = theme.accent;
    let fg = if !allowed {
        theme.error
    } else if effect.is_move() {
        accent
    } else {
        theme.text_strong
    };

    egui::Area::new(egui::Id::new("file-drag-badge"))
        .order(egui::Order::Tooltip)
        .fixed_pos(pos + egui::vec2(18.0, 20.0))
        .interactable(false)
        .show(ctx, |ui| {
            let mut frame = egui::Frame::popup(ui.style());
            if effect.is_move() && allowed {
                frame = frame
                    .fill(accent.gamma_multiply(0.22))
                    .stroke(egui::Stroke::new(1.0, accent));
            }
            frame
                .inner_margin(egui::Margin::symmetric(8, 5))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 6.0;
                        let (icon_rect, _) =
                            ui.allocate_exact_size(egui::vec2(16.0, 16.0), egui::Sense::hover());
                        crate::icons::paint_glyph(ui.painter(), icon_rect, effect.glyph(), fg);
                        let title = match hint {
                            Some(h) if !h.allowed => format!("Can't drop {noun} here"),
                            Some(h) => format!("{} {noun} to {}", effect.verb(), h.dest),
                            None => format!("{} {noun}", effect.verb()),
                        };
                        ui.label(egui::RichText::new(title).strong().color(fg));
                    });
                    if allowed && !effect.is_move() {
                        ui.weak("Hold Ctrl or Shift to move");
                    }
                });
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn marquee(anchor_content_y: f32) -> Marquee {
        Marquee {
            pane: 0,
            tab_uid: 1,
            anchor_content_y,
            anchor_x: 0.0,
            additive: false,
            keep: HashSet::new(),
        }
    }

    /// Rows 20pt tall with no spacing: row 0 is [0,20), row 1 is [20,40), ...
    fn m20() -> RowMetrics {
        RowMetrics {
            origin_y: 0.0,
            pitch: 20.0,
        }
    }

    #[test]
    fn band_covers_the_rows_it_spans() {
        assert_eq!(marquee(10.0).rows(50.0, m20(), 10), Some((0, 2)));
    }

    #[test]
    fn dragging_upward_gives_the_same_range() {
        let down = marquee(10.0).rows(50.0, m20(), 10);
        let up = marquee(50.0).rows(10.0, m20(), 10);
        assert_eq!(down, up);
    }

    #[test]
    fn band_reaches_rows_scrolled_out_of_view() {
        // Content space is scroll-independent, so a band ending far down the
        // list selects those rows even though they were never drawn.
        assert_eq!(marquee(0.0).rows(1_999.0, m20(), 100), Some((0, 99)));
    }

    #[test]
    fn band_past_the_last_row_clamps_instead_of_wrapping() {
        assert_eq!(marquee(10.0).rows(10_000.0, m20(), 3), Some((0, 2)));
    }

    #[test]
    fn band_entirely_above_the_list_selects_nothing() {
        assert_eq!(marquee(-80.0).rows(-40.0, m20(), 10), None);
    }

    #[test]
    fn empty_list_selects_nothing() {
        assert_eq!(marquee(0.0).rows(100.0, m20(), 0), None);
    }

    #[test]
    fn a_click_with_no_drag_still_covers_its_row() {
        // Anchor and cursor on the same point inside row 3.
        assert_eq!(marquee(70.0).rows(70.0, m20(), 10), Some((3, 3)));
    }

    // ---- row metric calibration ----
    //
    // These pin down the bug that made long drags under-select: the row pitch
    // is `row_height + item_spacing.y`, so using the bare row height loses a
    // fraction of a row per row and skips rows as the drag gets longer.

    #[test]
    fn metrics_measure_pitch_including_spacing() {
        // 18pt rows with 3pt spacing => tops 21pt apart.
        let samples = [(0usize, 100.0f32), (1, 121.0), (2, 142.0)];
        let m = RowMetrics::from_rows(&samples, 18.0);
        assert!((m.pitch - 21.0).abs() < 0.01, "pitch was {}", m.pitch);
        assert!((m.origin_y - 100.0).abs() < 0.01);
    }

    #[test]
    fn metrics_recover_origin_when_scrolled() {
        // Row 40 is the first one drawn, 10pt below the viewport top.
        let samples = [(40usize, 10.0f32), (41, 31.0)];
        let m = RowMetrics::from_rows(&samples, 18.0);
        assert!((m.pitch - 21.0).abs() < 0.01);
        // Content y 0 is therefore 40 rows above the visible area.
        assert!((m.origin_y - (10.0 - 40.0 * 21.0)).abs() < 0.01);
        // Round-trip: row 40's top maps back to content y 40*21.
        assert!((m.to_content(10.0) - 40.0 * 21.0).abs() < 0.01);
    }

    #[test]
    fn using_the_true_pitch_selects_every_row_in_a_long_band() {
        // 18pt rows, 3pt spacing, dragging from row 0 through all of row 49.
        let m = RowMetrics {
            origin_y: 0.0,
            pitch: 21.0,
        };
        let band_bottom = 49.0 * 21.0 + 20.0; // just inside row 49
        assert_eq!(marquee(0.0).rows(band_bottom, m, 200), Some((0, 49)));

        // The old code used the bare row height as the pitch. Same pixels,
        // wrong pitch => the range runs far past where the user actually is,
        // which is the same miscalibration that skipped rows in the other
        // direction once the origin was off too.
        let wrong = RowMetrics {
            origin_y: 0.0,
            pitch: 18.0,
        };
        assert_ne!(marquee(0.0).rows(band_bottom, wrong, 200), Some((0, 49)));
    }

    #[test]
    fn metrics_fall_back_when_only_one_row_is_visible() {
        let m = RowMetrics::from_rows(&[(3usize, 63.0f32)], 21.0);
        assert!((m.pitch - 21.0).abs() < 0.01);
        assert!((m.origin_y - 0.0).abs() < 0.01);
    }

    #[test]
    fn metrics_survive_an_empty_list() {
        let m = RowMetrics::from_rows(&[], 21.0);
        assert!((m.pitch - 21.0).abs() < 0.01);
    }

    #[test]
    fn metrics_reject_a_degenerate_pitch() {
        // Collapsed rows (all tops equal) must not yield a zero pitch, which
        // would divide by zero when mapping content y to a row.
        let m = RowMetrics::from_rows(&[(0usize, 50.0f32), (1, 50.0)], 21.0);
        assert!(m.pitch > 0.5, "pitch was {}", m.pitch);
    }

    #[test]
    fn drop_rejects_no_op_and_recursive_targets() {
        let dest = Path::new(r"C:\dst");
        // Already in the destination.
        assert!(!drop_allowed(&[PathBuf::from(r"C:\dst\a.txt")], dest));
        // The destination itself.
        assert!(!drop_allowed(&[PathBuf::from(r"C:\dst")], dest));
        // A parent of the destination: would nest a folder inside itself.
        assert!(!drop_allowed(&[PathBuf::from(r"C:\")], dest));
        // A genuine move from elsewhere.
        assert!(drop_allowed(&[PathBuf::from(r"C:\src\a.txt")], dest));
        // Mixed: allowed as long as one source can land.
        assert!(drop_allowed(
            &[
                PathBuf::from(r"C:\dst\a.txt"),
                PathBuf::from(r"C:\src\b.txt")
            ],
            dest
        ));
    }
}
