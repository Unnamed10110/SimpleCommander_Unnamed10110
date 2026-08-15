//! Token-based theming. The flagship "AMOLED" theme uses pure #000000
//! surfaces everywhere (true black for OLED panels), hairline separators,
//! and a single configurable accent.

use egui::{Color32, CornerRadius, Stroke, Visuals};

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Theme {
    pub name: &'static str,
    pub bg: Color32,
    pub panel: Color32,
    pub header: Color32,
    pub separator: Color32,
    pub text: Color32,
    pub text_weak: Color32,
    pub text_strong: Color32,
    pub accent: Color32,
    pub accent_dim: Color32,
    pub selection_bg: Color32,
    pub hover_bg: Color32,
    pub stripe: Color32,
    pub folder: Color32,
    pub error: Color32,
    pub warn: Color32,
    pub ok: Color32,
}

pub const AMOLED: Theme = Theme {
    name: "amoled",
    bg: Color32::BLACK,
    panel: Color32::BLACK,
    header: Color32::BLACK,
    separator: Color32::from_rgb(0x1a, 0x1a, 0x1a),
    text: Color32::from_rgb(0xd4, 0xd4, 0xd4),
    text_weak: Color32::from_rgb(0x8a, 0x8a, 0x8a),
    text_strong: Color32::from_rgb(0xff, 0xff, 0xff),
    accent: Color32::from_rgb(0x2f, 0xb8, 0xff),
    accent_dim: Color32::from_rgb(0x14, 0x53, 0x73),
    selection_bg: Color32::from_rgb(0x0a, 0x2b, 0x3f),
    hover_bg: Color32::from_rgb(0x0e, 0x0e, 0x0e),
    stripe: Color32::from_rgb(0x05, 0x05, 0x05),
    folder: Color32::from_rgb(0xff, 0xd8, 0x66),
    error: Color32::from_rgb(0xff, 0x5c, 0x5c),
    warn: Color32::from_rgb(0xff, 0xb8, 0x4d),
    ok: Color32::from_rgb(0x51, 0xd8, 0x8a),
};

pub const DARK: Theme = Theme {
    name: "dark",
    bg: Color32::from_rgb(0x1b, 0x1d, 0x1f),
    panel: Color32::from_rgb(0x20, 0x22, 0x25),
    header: Color32::from_rgb(0x25, 0x27, 0x2a),
    separator: Color32::from_rgb(0x33, 0x36, 0x3a),
    text: Color32::from_rgb(0xd4, 0xd4, 0xd4),
    text_weak: Color32::from_rgb(0x93, 0x96, 0x9a),
    text_strong: Color32::from_rgb(0xff, 0xff, 0xff),
    accent: Color32::from_rgb(0x4d, 0xa6, 0xff),
    accent_dim: Color32::from_rgb(0x1d, 0x46, 0x6e),
    selection_bg: Color32::from_rgb(0x1d, 0x3a, 0x5c),
    hover_bg: Color32::from_rgb(0x2a, 0x2d, 0x31),
    stripe: Color32::from_rgb(0x1e, 0x20, 0x23),
    folder: Color32::from_rgb(0xff, 0xd8, 0x66),
    error: Color32::from_rgb(0xff, 0x5c, 0x5c),
    warn: Color32::from_rgb(0xff, 0xb8, 0x4d),
    ok: Color32::from_rgb(0x51, 0xd8, 0x8a),
};

pub fn by_name(name: &str) -> Theme {
    match name {
        "dark" => DARK,
        _ => AMOLED,
    }
}

/// Label palette (XYplorer-style colored labels), index 0 = none.
pub const LABEL_COLORS: [(&str, Color32); 8] = [
    ("None", Color32::TRANSPARENT),
    ("Red", Color32::from_rgb(0xe8, 0x4a, 0x4a)),
    ("Orange", Color32::from_rgb(0xf0, 0x94, 0x40)),
    ("Yellow", Color32::from_rgb(0xe8, 0xd4, 0x4a)),
    ("Green", Color32::from_rgb(0x4a, 0xc8, 0x6a)),
    ("Blue", Color32::from_rgb(0x4a, 0x90, 0xe8)),
    ("Purple", Color32::from_rgb(0xa8, 0x6a, 0xe8)),
    ("Gray", Color32::from_rgb(0x90, 0x90, 0x90)),
];

/// Register Windows symbol fonts as fallbacks so glyphs like arrows,
/// geometric shapes and UI symbols render instead of placeholder squares.
/// egui only bundles Ubuntu/Hack/NotoEmoji, which miss many of them.
pub fn install_font_fallbacks(ctx: &egui::Context) {
    let windir = std::env::var_os("WINDIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from("C:\\Windows"));
    let fonts_dir = windir.join("Fonts");

    let mut fonts = egui::FontDefinitions::default();
    let mut any = false;
    // Segoe UI Symbol covers Geometric Shapes / Arrows / Misc Technical;
    // Segoe MDL2 Assets adds modern Windows UI icons. Both ship with Windows.
    for (name, file) in [("seguisym", "seguisym.ttf"), ("segmdl2", "segmdl2.ttf")] {
        let Ok(bytes) = std::fs::read(fonts_dir.join(file)) else { continue };
        fonts
            .font_data
            .insert(name.to_owned(), egui::FontData::from_owned(bytes).into());
        for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
            fonts.families.entry(family).or_default().push(name.to_owned());
        }
        any = true;
    }
    if any {
        ctx.set_fonts(fonts);
    }
}

/// Parse an RGB hex string ("2fb8ff" or "#2fb8ff") into a color.
pub fn parse_hex(s: &str) -> Option<Color32> {
    let s = s.trim().trim_start_matches('#');
    if s.len() != 6 {
        return None;
    }
    let n = u32::from_str_radix(s, 16).ok()?;
    Some(Color32::from_rgb((n >> 16) as u8, (n >> 8) as u8, n as u8))
}

pub fn hex_of(c: Color32) -> String {
    format!("{:02x}{:02x}{:02x}", c.r(), c.g(), c.b())
}

/// Override accent / selection colors from a user hex string. Empty = no-op.
pub fn apply_accent_override(t: &mut Theme, hex: &str) {
    let Some(c) = parse_hex(hex) else { return };
    t.accent = c;
    t.accent_dim = Color32::from_rgb(c.r() / 3, c.g() / 3, c.b() / 3);
    t.selection_bg = Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), 48);
}

/// Apply the theme to the egui context.
pub fn apply(ctx: &egui::Context, t: &Theme) {
    let mut v = Visuals::dark();
    v.override_text_color = Some(t.text);
    v.panel_fill = t.panel;
    v.window_fill = t.bg;
    v.extreme_bg_color = t.bg;
    v.faint_bg_color = t.stripe;
    v.window_stroke = Stroke::new(1.0, t.separator);
    v.selection.bg_fill = t.selection_bg;
    v.selection.stroke = Stroke::new(1.0, t.accent);
    v.hyperlink_color = t.accent;
    v.warn_fg_color = t.warn;
    v.error_fg_color = t.error;

    let corner = CornerRadius::same(3);
    for (wv, bg, stroke_c) in [
        (&mut v.widgets.noninteractive, t.panel, t.separator),
        (&mut v.widgets.inactive, t.hover_bg, t.separator),
        (&mut v.widgets.hovered, t.selection_bg, t.accent_dim),
        (&mut v.widgets.active, t.selection_bg, t.accent),
        (&mut v.widgets.open, t.selection_bg, t.accent_dim),
    ] {
        wv.bg_fill = bg;
        wv.weak_bg_fill = bg;
        wv.bg_stroke = Stroke::new(1.0, stroke_c);
        wv.corner_radius = corner;
    }
    v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, t.text);
    v.widgets.inactive.fg_stroke = Stroke::new(1.0, t.text);
    v.widgets.hovered.fg_stroke = Stroke::new(1.0, t.text_strong);
    v.widgets.active.fg_stroke = Stroke::new(1.0, t.text_strong);
    v.widgets.open.fg_stroke = Stroke::new(1.0, t.text_strong);

    // Force dark mode and apply to all styles. No animations that would
    // force continuous repaints.
    ctx.set_theme(egui::Theme::Dark);
    ctx.all_styles_mut(|style| {
        style.visuals = v.clone();
        style.animation_time = 0.0;
        style.interaction.selectable_labels = false;
        style.spacing.item_spacing = egui::vec2(6.0, 4.0);
        style.spacing.button_padding = egui::vec2(8.0, 3.0);
    });
}
