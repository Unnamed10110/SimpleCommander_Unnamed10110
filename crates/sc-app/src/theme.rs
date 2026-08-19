//! Token-based theming. AMOLED palettes use pure #000000 surfaces
//! (true black for OLED panels). Light palettes use Visuals::light().

use egui::{Color32, CornerRadius, Stroke, Visuals};

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Theme {
    pub name: &'static str,
    pub dark: bool,
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
    dark: true,
    bg: Color32::BLACK,
    panel: Color32::BLACK,
    header: Color32::BLACK,
    separator: Color32::from_rgb(0x1f, 0x33, 0x3d),
    text: Color32::from_rgb(0xd4, 0xd4, 0xd4),
    text_weak: Color32::from_rgb(0x8a, 0x8a, 0x8a),
    text_strong: Color32::from_rgb(0xff, 0xff, 0xff),
    accent: Color32::from_rgb(0x2f, 0xb8, 0xff),
    accent_dim: Color32::from_rgb(0x14, 0x53, 0x73),
    selection_bg: Color32::from_rgb(0x0a, 0x2b, 0x3f),
    hover_bg: Color32::from_rgb(0x0a, 0x1e, 0x29),
    stripe: Color32::from_rgb(0x06, 0x0e, 0x13),
    folder: Color32::from_rgb(0xff, 0xd8, 0x66),
    error: Color32::from_rgb(0xff, 0x5c, 0x5c),
    warn: Color32::from_rgb(0xff, 0xb8, 0x4d),
    ok: Color32::from_rgb(0x51, 0xd8, 0x8a),
};

pub const AMOLED_AMBER: Theme = Theme {
    name: "amoled_amber",
    dark: true,
    bg: Color32::BLACK,
    panel: Color32::BLACK,
    header: Color32::BLACK,
    separator: Color32::from_rgb(0x33, 0x26, 0x0f),
    text: Color32::from_rgb(0xe8, 0xdc, 0xc8),
    text_weak: Color32::from_rgb(0x9a, 0x86, 0x68),
    text_strong: Color32::from_rgb(0xff, 0xf4, 0xe0),
    accent: Color32::from_rgb(0xff, 0xb0, 0x20),
    accent_dim: Color32::from_rgb(0x6a, 0x42, 0x08),
    selection_bg: Color32::from_rgb(0x2a, 0x18, 0x00),
    hover_bg: Color32::from_rgb(0x23, 0x18, 0x04),
    stripe: Color32::from_rgb(0x12, 0x0c, 0x03),
    folder: Color32::from_rgb(0xff, 0xd8, 0x66),
    error: Color32::from_rgb(0xff, 0x6b, 0x5c),
    warn: Color32::from_rgb(0xff, 0xb8, 0x4d),
    ok: Color32::from_rgb(0x7a, 0xd8, 0x6a),
};

pub const AMOLED_VIOLET: Theme = Theme {
    name: "amoled_violet",
    dark: true,
    bg: Color32::BLACK,
    panel: Color32::BLACK,
    header: Color32::BLACK,
    separator: Color32::from_rgb(0x2a, 0x1f, 0x3d),
    text: Color32::from_rgb(0xdc, 0xd4, 0xea),
    text_weak: Color32::from_rgb(0x8a, 0x7c, 0xa0),
    text_strong: Color32::from_rgb(0xf6, 0xf0, 0xff),
    accent: Color32::from_rgb(0xc4, 0x84, 0xfc),
    accent_dim: Color32::from_rgb(0x4c, 0x1d, 0x95),
    selection_bg: Color32::from_rgb(0x1e, 0x0a, 0x3a),
    hover_bg: Color32::from_rgb(0x1b, 0x12, 0x23),
    stripe: Color32::from_rgb(0x0d, 0x09, 0x11),
    folder: Color32::from_rgb(0xff, 0xd8, 0x66),
    error: Color32::from_rgb(0xff, 0x6b, 0x8a),
    warn: Color32::from_rgb(0xff, 0xb8, 0x4d),
    ok: Color32::from_rgb(0x5e, 0xe0, 0xb0),
};

pub const DARK: Theme = Theme {
    name: "dark",
    dark: true,
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

pub const LIGHT: Theme = Theme {
    name: "light",
    dark: false,
    bg: Color32::from_rgb(0xf4, 0xf6, 0xf8),
    panel: Color32::from_rgb(0xff, 0xff, 0xff),
    header: Color32::from_rgb(0xec, 0xef, 0xf3),
    separator: Color32::from_rgb(0xd0, 0xd5, 0xdc),
    text: Color32::from_rgb(0x1c, 0x1f, 0x24),
    text_weak: Color32::from_rgb(0x6b, 0x72, 0x80),
    text_strong: Color32::from_rgb(0x0b, 0x0d, 0x10),
    accent: Color32::from_rgb(0x1d, 0x6b, 0xe0),
    accent_dim: Color32::from_rgb(0xa8, 0xc8, 0xf0),
    selection_bg: Color32::from_rgb(0xd6, 0xe6, 0xfb),
    hover_bg: Color32::from_rgb(0xe8, 0xec, 0xf1),
    stripe: Color32::from_rgb(0xee, 0xf1, 0xf5),
    folder: Color32::from_rgb(0xc9, 0x8a, 0x1a),
    error: Color32::from_rgb(0xdc, 0x26, 0x26),
    warn: Color32::from_rgb(0xd9, 0x77, 0x06),
    ok: Color32::from_rgb(0x05, 0x96, 0x69),
};

pub const LIGHT_WARM: Theme = Theme {
    name: "light_warm",
    dark: false,
    bg: Color32::from_rgb(0xf7, 0xf1, 0xe8),
    panel: Color32::from_rgb(0xff, 0xfb, 0xf5),
    header: Color32::from_rgb(0xf0, 0xe6, 0xd6),
    separator: Color32::from_rgb(0xdf, 0xd2, 0xbe),
    text: Color32::from_rgb(0x2c, 0x24, 0x18),
    text_weak: Color32::from_rgb(0x7a, 0x6a, 0x56),
    text_strong: Color32::from_rgb(0x1a, 0x12, 0x0a),
    accent: Color32::from_rgb(0xc4, 0x5c, 0x26),
    accent_dim: Color32::from_rgb(0xe8, 0xb8, 0x98),
    selection_bg: Color32::from_rgb(0xf5, 0xdc, 0xc8),
    hover_bg: Color32::from_rgb(0xef, 0xe4, 0xd4),
    stripe: Color32::from_rgb(0xf3, 0xea, 0xdc),
    folder: Color32::from_rgb(0xb4, 0x53, 0x09),
    error: Color32::from_rgb(0xb9, 0x1c, 0x1c),
    warn: Color32::from_rgb(0xb4, 0x53, 0x09),
    ok: Color32::from_rgb(0x04, 0x78, 0x57),
};

/// `(id, label)` pairs shown in View → Theme and Settings → Appearance.
pub fn catalog() -> &'static [(&'static str, &'static str)] {
    &[
        ("amoled", "AMOLED"),
        ("dark", "Dark"),
        ("light", "Light"),
        ("light_warm", "Light Warm"),
    ]
}

pub fn is_amoled(name: &str) -> bool {
    name.starts_with("amoled")
}

/// Named accent chips shown next to AMOLED (and usable on any theme).
pub const ACCENT_PRESETS: &[(&str, Color32)] = &[
    ("Cyan", Color32::from_rgb(0x2f, 0xb8, 0xff)),
    ("Amber", Color32::from_rgb(0xff, 0xb0, 0x20)),
    ("Violet", Color32::from_rgb(0xc4, 0x84, 0xfc)),
    ("Red", Color32::from_rgb(0xff, 0x4d, 0x4d)),
    ("Orange", Color32::from_rgb(0xff, 0x7a, 0x1a)),
    ("Green", Color32::from_rgb(0x3d, 0xd6, 0x8c)),
    ("Pink", Color32::from_rgb(0xff, 0x5c, 0xa8)),
    ("White", Color32::from_rgb(0xe8, 0xe8, 0xe8)),
];

pub fn by_name(name: &str) -> Theme {
    match name {
        "amoled_amber" => AMOLED_AMBER,
        "amoled_violet" => AMOLED_VIOLET,
        "dark" => DARK,
        "light" => LIGHT,
        "light_warm" => LIGHT_WARM,
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

fn scale_rgb(c: Color32, factor: f32) -> Color32 {
    Color32::from_rgb(
        ((c.r() as f32) * factor).round().clamp(0.0, 255.0) as u8,
        ((c.g() as f32) * factor).round().clamp(0.0, 255.0) as u8,
        ((c.b() as f32) * factor).round().clamp(0.0, 255.0) as u8,
    )
}

/// Blend `a` toward `b` by `t` (0 = all `a`, 1 = all `b`).
pub fn mix_rgb(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    Color32::from_rgb(
        (a.r() as f32 + (b.r() as f32 - a.r() as f32) * t).round() as u8,
        (a.g() as f32 + (b.g() as f32 - a.g() as f32) * t).round() as u8,
        (a.b() as f32 + (b.b() as f32 - a.b() as f32) * t).round() as u8,
    )
}

/// Override accent / selection colors from a user hex string. Empty = no-op.
/// On AMOLED this retints hover, selection, and separators so the whole
/// chrome follows the chosen color (cyan → red, etc.).
pub fn apply_accent_override(t: &mut Theme, hex: &str) {
    let Some(c) = parse_hex(hex) else { return };
    t.accent = c;
    if t.dark {
        t.accent_dim = scale_rgb(c, 0.38);
        t.selection_bg = scale_rgb(c, 0.20);
        if t.bg == Color32::BLACK {
            // Scaling the accent toward black yields a tinted dark surface. The
            // old factors (0.07 / 0.04) landed under about 20/255 on the
            // brightest channel, which is not perceptible on an OLED panel.
            t.hover_bg = scale_rgb(c, 0.14);
            t.stripe = scale_rgb(c, 0.07);
            t.separator = mix_rgb(Color32::from_rgb(0x1a, 0x1a, 0x1a), c, 0.22);
        } else {
            t.hover_bg = mix_rgb(t.hover_bg, c, 0.14);
        }
    } else {
        t.accent_dim = mix_rgb(c, Color32::WHITE, 0.55);
        t.selection_bg = mix_rgb(c, Color32::WHITE, 0.80);
    }
}

/// Preset chips plus a custom color picker. Writes an RGB hex string
/// (no `#`) into `accent_hex`. Returns true if the value changed.
pub fn accent_editor(ui: &mut egui::Ui, accent_hex: &mut String, current: Color32) -> bool {
    let mut changed = false;
    ui.horizontal_wrapped(|ui| {
        for (name, color) in ACCENT_PRESETS {
            let selected = current == *color;
            if color_swatch(ui, *color, selected, name).clicked() {
                *accent_hex = hex_of(*color);
                changed = true;
            }
        }
        let mut rgb = [current.r(), current.g(), current.b()];
        if ui
            .color_edit_button_srgb(&mut rgb)
            .on_hover_text("Custom color")
            .changed()
        {
            *accent_hex = hex_of(Color32::from_rgb(rgb[0], rgb[1], rgb[2]));
            changed = true;
        }
        ui.weak(format!("#{}", hex_of(current)));
        if !accent_hex.is_empty() && ui.small_button("Reset").clicked() {
            accent_hex.clear();
            changed = true;
        }
    });
    changed
}

fn color_swatch(ui: &mut egui::Ui, color: Color32, selected: bool, tip: &str) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(18.0, 18.0), egui::Sense::click());
    let stroke = if selected {
        Stroke::new(2.0, Color32::WHITE)
    } else {
        Stroke::new(1.0, Color32::from_gray(70))
    };
    ui.painter().rect(
        rect.shrink(1.0),
        3.0,
        color,
        stroke,
        egui::StrokeKind::Inside,
    );
    resp.on_hover_text(tip)
}

/// Apply the theme to the egui context.
pub fn apply(ctx: &egui::Context, t: &Theme) {
    let mut v = if t.dark {
        Visuals::dark()
    } else {
        Visuals::light()
    };
    v.dark_mode = t.dark;
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

    // One radius system: 4 for controls and rows, 8 for floating surfaces.
    let corner = CornerRadius::same(4);
    v.window_corner_radius = CornerRadius::same(8);
    v.menu_corner_radius = CornerRadius::same(8);
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

    ctx.set_theme(if t.dark {
        egui::Theme::Dark
    } else {
        egui::Theme::Light
    });
    ctx.all_styles_mut(|style| {
        style.visuals = v.clone();
        // Short enough to stay snappy, long enough that hover and expand read as
        // movement rather than teleporting. Zero here made the whole UI feel
        // brittle; the file table sets its own spacing/feel separately.
        style.animation_time = 0.08;
        style.interaction.selectable_labels = false;
        style.spacing.item_spacing = egui::vec2(6.0, 4.0);
        style.spacing.button_padding = egui::vec2(8.0, 3.0);
    });
}
