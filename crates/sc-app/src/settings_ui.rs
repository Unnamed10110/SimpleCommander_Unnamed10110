//! Settings window: left category list, right editor. Changes apply immediately
//! and are written to settings.toml.

use crate::app::{replace_cmdline_program, ScApp};
use crate::config::{ColorRule, ConflictDefault, DefaultLayout, PreviewPlacement};
use crate::keymap::{Keymap, SHORTCUT_ROWS};
use crate::theme;
use egui::{Align2, Color32, Slider, TextEdit};

const CATS: &[&str] = &[
    "Appearance",
    "Behavior",
    "Shortcuts",
    "Panes & tabs",
    "File operations",
    "Search & index",
    "Color rules",
    "Advanced",
];

pub fn draw(app: &mut ScApp, ctx: &egui::Context) {
    if !app.show_settings {
        return;
    }
    let mut open = app.show_settings;
    let mut persist = false;
    let mut appearance = false;
    let mut rebuild = false;

    egui::Window::new("Settings")
        .open(&mut open)
        .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
        .default_size([720.0, 480.0])
        .resizable(true)
        .show(ctx, |ui| {
            ui.horizontal_top(|ui| {
                ui.vertical(|ui| {
                    ui.set_min_width(140.0);
                    ui.set_max_width(160.0);
                    for (i, label) in CATS.iter().enumerate() {
                        if ui
                            .selectable_label(app.settings_cat == i, *label)
                            .clicked()
                        {
                            app.settings_cat = i;
                        }
                    }
                });
                ui.separator();
                ui.vertical(|ui| {
                    egui::ScrollArea::vertical()
                        .auto_shrink([false, false])
                        .show(ui, |ui| match app.settings_cat {
                            0 => appearance_page(app, ui, &mut persist, &mut appearance, &mut rebuild),
                            1 => behavior_page(app, ui, &mut persist, &mut rebuild),
                            2 => shortcuts_page(app, ui, &mut persist),
                            3 => panes_page(app, ui, &mut persist),
                            4 => ops_page(app, ui, &mut persist),
                            5 => search_page(app, ui, &mut persist),
                            6 => color_rules_page(app, ui, &mut persist),
                            _ => advanced_page(app, ui),
                        });
                });
            });
        });

    app.show_settings = open;
    if !open {
        app.capture_shortcut = None;
    }
    if persist {
        app.persist_settings();
    }
    if appearance {
        app.apply_appearance(ctx);
    }
    if rebuild {
        app.rebuild_view(0);
        app.rebuild_view(1);
    }
}

fn appearance_page(
    app: &mut ScApp,
    ui: &mut egui::Ui,
    persist: &mut bool,
    appearance: &mut bool,
    rebuild: &mut bool,
) {
    ui.heading("Appearance");
    ui.add_space(6.0);

    ui.label("Theme");
    for (id, label) in theme::catalog() {
        let selected = if *id == "amoled" {
            theme::is_amoled(&app.settings.session.theme)
        } else {
            app.settings.session.theme == *id
        };
        if ui.radio(selected, *label).clicked() {
            app.set_theme(id);
            *persist = true;
            *appearance = true;
        }
    }
    ui.add_space(8.0);

    ui.label(if theme::is_amoled(&app.settings.session.theme) {
        "AMOLED color"
    } else {
        "Accent color"
    });
    let mut hex = app.settings.accent.clone();
    if theme::accent_editor(ui, &mut hex, app.theme.accent) {
        app.set_accent(&hex);
        *persist = true;
        *appearance = true;
    }
    ui.add_space(8.0);

    ui.label("UI scale");
    let mut scale = app.settings.ui_scale;
    if ui
        .add(Slider::new(&mut scale, 0.75..=2.0).suffix("×").fixed_decimals(2))
        .changed()
    {
        app.settings.ui_scale = scale;
        *persist = true;
        *appearance = true;
    }

    ui.label("Row height");
    let mut rh = app.settings.row_height;
    if ui
        .add(Slider::new(&mut rh, 16.0..=36.0).suffix(" px").fixed_decimals(0))
        .changed()
    {
        app.settings.row_height = rh;
        *persist = true;
    }

    if ui
        .checkbox(&mut app.settings.striped_rows, "Striped file list rows")
        .changed()
    {
        *persist = true;
    }
    if ui
        .checkbox(&mut app.settings.show_icons, "Show file icons")
        .changed()
    {
        *persist = true;
        *rebuild = true;
    }

    ui.add_space(8.0);
    ui.label("Preview pane");
    let place = app.settings.preview_placement;
    for opt in [
        PreviewPlacement::Floating,
        PreviewPlacement::Right,
        PreviewPlacement::Bottom,
    ] {
        if ui.radio(place == opt, opt.label()).clicked() {
            app.settings.preview_placement = opt;
            *persist = true;
        }
    }
}

fn behavior_page(app: &mut ScApp, ui: &mut egui::Ui, persist: &mut bool, rebuild: &mut bool) {
    ui.heading("Behavior");
    ui.add_space(6.0);

    let mut hidden = app.show_hidden;
    if ui.checkbox(&mut hidden, "Show hidden files").changed() {
        app.show_hidden = hidden;
        app.settings.session.show_hidden = hidden;
        *rebuild = true;
        *persist = true;
    }
    if ui
        .checkbox(&mut app.settings.single_click_open, "Open files and folders with a single click")
        .changed()
    {
        *persist = true;
    }
    let perm_label = format!(
        "Confirm permanent delete ({})",
        app.settings.keymap.delete_permanent.label()
    );
    if ui
        .checkbox(&mut app.settings.confirm_permanent_delete, perm_label)
        .changed()
    {
        *persist = true;
    }
    let rec_label = format!(
        "Confirm recycle-bin delete ({})",
        app.settings.keymap.delete.label()
    );
    if ui
        .checkbox(&mut app.settings.confirm_recycle_delete, rec_label)
        .changed()
    {
        *persist = true;
    }

    ui.add_space(8.0);
    ui.label("Type-ahead timeout");
    let mut ms = app.settings.type_ahead_ms as f64;
    if ui
        .add(Slider::new(&mut ms, 200.0..=2000.0).suffix(" ms").fixed_decimals(0))
        .changed()
    {
        app.settings.type_ahead_ms = ms as u64;
        *persist = true;
    }

    ui.label("Session autosave interval");
    let mut secs = app.settings.autosave_secs as f64;
    if ui
        .add(Slider::new(&mut secs, 5.0..=300.0).suffix(" s").fixed_decimals(0))
        .changed()
    {
        app.settings.autosave_secs = secs as u64;
        *persist = true;
    }

    ui.add_space(12.0);
    terminal_command_editor(app, ui, persist);
}

fn terminal_command_editor(app: &mut ScApp, ui: &mut egui::Ui, persist: &mut bool) {
    ui.label("Terminal / program");
    ui.weak("Used by the toolbar button and the Open terminal shortcut. `{path}` is replaced with the active folder.");
    ui.horizontal(|ui| {
        if ui
            .add(
                TextEdit::singleline(&mut app.settings.terminal_command).desired_width(360.0),
            )
            .changed()
        {
            *persist = true;
        }
        if ui.button("Browse…").clicked() {
            if let Some(path) = rfd::FileDialog::new()
                .add_filter("Programs", &["exe", "bat", "cmd", "ps1"])
                .pick_file()
            {
                let next = replace_cmdline_program(
                    &app.settings.terminal_command,
                    &path.display().to_string(),
                );
                app.settings.terminal_command = next;
                *persist = true;
            }
        }
    });
    ui.horizontal(|ui| {
        ui.weak("Presets:");
        if ui.small_button("Windows Terminal").clicked() {
            app.settings.terminal_command = "wt.exe -d \"{path}\"".into();
            *persist = true;
        }
        if ui.small_button("Command Prompt").clicked() {
            app.settings.terminal_command = "cmd.exe /k cd /d \"{path}\"".into();
            *persist = true;
        }
        if ui.small_button("PowerShell").clicked() {
            app.settings.terminal_command =
                "powershell.exe -NoExit -Command Set-Location -LiteralPath \"{path}\"".into();
            *persist = true;
        }
    });
}

fn shortcuts_page(app: &mut ScApp, ui: &mut egui::Ui, persist: &mut bool) {
    ui.heading("Shortcuts");
    ui.add_space(6.0);
    terminal_command_editor(app, ui, persist);

    ui.add_space(16.0);
    ui.label("Keyboard shortcuts");
    ui.weak("Click a shortcut, then press the new keys. Esc cancels. List navigation (Up/Down/Page/Home/End) is not remappable.");
    ui.add_space(8.0);

    egui::Grid::new("shortcut-grid")
        .num_columns(3)
        .spacing([12.0, 4.0])
        .striped(true)
        .show(ui, |ui| {
            ui.strong("Action");
            ui.strong("Shortcut");
            ui.label("");
            ui.end_row();

            for &(id, label) in SHORTCUT_ROWS {
                ui.label(label);
                let capturing = app.capture_shortcut == Some(id);
                let text = if capturing {
                    "Press a key…".to_string()
                } else {
                    app.settings.keymap.get(id).label()
                };
                let mut btn = egui::Button::new(text).min_size(egui::vec2(140.0, 0.0));
                if capturing {
                    btn = btn.fill(ui.visuals().selection.bg_fill);
                }
                if ui.add(btn).clicked() {
                    app.capture_shortcut = if capturing { None } else { Some(id) };
                }
                if ui.small_button("Reset").clicked() {
                    let chord = Keymap::default().get(id).clone();
                    *app.settings.keymap.get_mut(id) = chord;
                    if app.capture_shortcut == Some(id) {
                        app.capture_shortcut = None;
                    }
                    *persist = true;
                }
                if let Some(other) = app.settings.keymap.conflicts_with(id) {
                    ui.end_row();
                    ui.label("");
                    ui.colored_label(
                        Color32::from_rgb(0xff, 0x88, 0x66),
                        format!("Also used by {other}"),
                    );
                    ui.label("");
                }
                ui.end_row();
            }
        });

    ui.add_space(10.0);
    if ui.button("Reset all shortcuts").clicked() {
        app.settings.keymap = Keymap::default();
        app.capture_shortcut = None;
        *persist = true;
    }
}

fn panes_page(app: &mut ScApp, ui: &mut egui::Ui, persist: &mut bool) {
    ui.heading("Panes & tabs");
    ui.add_space(6.0);

    if ui
        .checkbox(
            &mut app.settings.restore_session,
            "Restore tabs, panes, and paths on start",
        )
        .changed()
    {
        *persist = true;
    }
    if ui
        .checkbox(
            &mut app.settings.remember_sort,
            "Remember per-tab sort order across restarts",
        )
        .changed()
    {
        *persist = true;
    }

    ui.add_space(8.0);
    ui.label("Default layout (used when session restore is off)");
    let current = app.settings.default_layout;
    ui.horizontal(|ui| {
        for opt in [
            DefaultLayout::DualVertical,
            DefaultLayout::DualHorizontal,
            DefaultLayout::Single,
        ] {
            if ui.radio(current == opt, opt.label()).clicked() {
                app.settings.default_layout = opt;
                app.layout = opt.to_pane_layout();
                *persist = true;
            }
        }
    });
    ui.weak("Changing this also switches the current window layout.");
}

fn ops_page(app: &mut ScApp, ui: &mut egui::Ui, persist: &mut bool) {
    ui.heading("File operations");
    ui.add_space(6.0);

    if ui
        .checkbox(
            &mut app.settings.delete_permanent_default,
            "Delete permanently by default (Del skips the recycle bin)",
        )
        .changed()
    {
        *persist = true;
    }

    ui.add_space(8.0);
    ui.label("When a copy/move target already exists");
    let current = app.settings.conflict_default;
    let opts = [
        (ConflictDefault::Ask, "Ask each time"),
        (ConflictDefault::Overwrite, "Overwrite"),
        (ConflictDefault::KeepBoth, "Keep both (auto-rename)"),
        (ConflictDefault::Skip, "Skip"),
    ];
    for (opt, label) in opts {
        if ui.radio(current == opt, label).clicked() {
            app.settings.conflict_default = opt;
            *persist = true;
        }
    }

    ui.add_space(10.0);
    ui.label("Parallel transfers");
    let mut jobs = app.settings.transfer_jobs.clamp(1, 4);
    if ui
        .add(egui::Slider::new(&mut jobs, 1..=4).text("worker threads"))
        .changed()
    {
        app.settings.transfer_jobs = jobs;
        app.ops.set_max_jobs(jobs as usize);
        *persist = true;
    }
    ui.weak("1 is strictly sequential. Overlapping paths still wait their turn.");
}

fn search_page(app: &mut ScApp, ui: &mut egui::Ui, persist: &mut bool) {
    ui.heading("Search & index");
    ui.add_space(6.0);

    if ui
        .checkbox(
            &mut app.settings.index_enabled,
            "Build the background filename index at startup",
        )
        .changed()
    {
        *persist = true;
    }
    ui.weak("Index on/off takes effect the next time SimpleCommander starts.");
    ui.add_space(6.0);
    if sc_shell::everything::is_running() {
        ui.label(format!(
            "Everything is running — {} uses its index.",
            app.settings.keymap.search.label()
        ));
    } else if sc_shell::everything::is_installed() {
        ui.weak("Everything is installed but not running. It will be started when you search.");
    } else {
        ui.weak(format!(
            "Everything is not installed. {} falls back to the built-in index.",
            app.settings.keymap.search.label()
        ));
        ui.add_space(4.0);
        if ui.button("Download Everything…").clicked() {
            sc_shell::everything::open_download_page();
        }
    }
    ui.add_space(8.0);
    ui.label("Everything.exe");
    ui.horizontal(|ui| {
        let mut path = app.settings.everything_exe.clone();
        if ui
            .add(TextEdit::singleline(&mut path).desired_width(360.0).hint_text("Auto-detect"))
            .changed()
        {
            app.settings.everything_exe = path;
            *persist = true;
        }
        if ui.button("Browse…").clicked() {
            if let Some(file) = rfd::FileDialog::new()
                .add_filter("Everything", &["exe"])
                .pick_file()
            {
                app.settings.everything_exe = file.display().to_string();
                *persist = true;
            }
        }
        if ui.small_button("Auto-detect").clicked() {
            app.settings.everything_exe.clear();
            *persist = true;
        }
    });
    if let Some(resolved) = sc_shell::everything::resolved_exe() {
        ui.weak(format!("Using {}", resolved.display()));
    } else if app.settings.everything_exe.trim().is_empty() {
        ui.weak("No Everything.exe found. Search will use the built-in index.");
    } else {
        ui.colored_label(
            Color32::from_rgb(0xff, 0x88, 0x66),
            "That path is not an existing .exe — auto-detect will be used if possible.",
        );
    }
    ui.add_space(6.0);
    let mut ask = !app.settings.everything_prompt_dismissed;
    if ui
        .checkbox(&mut ask, "Ask to install Everything at startup if it's missing")
        .changed()
    {
        app.settings.everything_prompt_dismissed = !ask;
        *persist = true;
    }

    ui.add_space(8.0);
    ui.label("Max name-search results");
    let mut max = app.settings.search_max_results as f64;
    if ui
        .add(Slider::new(&mut max, 50.0..=5000.0).fixed_decimals(0))
        .changed()
    {
        app.settings.search_max_results = max as usize;
        *persist = true;
    }

    ui.label("Content search skips files larger than");
    let mut mb = app.settings.content_search_max_mb as f64;
    if ui
        .add(Slider::new(&mut mb, 1.0..=256.0).suffix(" MB").fixed_decimals(0))
        .changed()
    {
        app.settings.content_search_max_mb = mb as u64;
        *persist = true;
    }
}

fn color_rules_page(app: &mut ScApp, ui: &mut egui::Ui, persist: &mut bool) {
    ui.heading("Color rules");
    ui.weak("Rows whose name matches a wildcard are tinted. First match wins.");
    ui.add_space(6.0);

    let mut remove: Option<usize> = None;
    let mut add = false;
    egui::Grid::new("color-rules")
        .num_columns(3)
        .spacing([8.0, 4.0])
        .show(ui, |ui| {
            ui.weak("Pattern");
            ui.weak("Color");
            ui.end_row();
            for (i, rule) in app.settings.color_rules.iter_mut().enumerate() {
                if ui
                    .add(TextEdit::singleline(&mut rule.pattern).desired_width(180.0))
                    .changed()
                {
                    *persist = true;
                }
                let mut rgb = theme::parse_hex(&rule.color)
                    .map(|c| [c.r(), c.g(), c.b()])
                    .unwrap_or([0xd4, 0xd4, 0xd4]);
                if ui.color_edit_button_srgb(&mut rgb).changed() {
                    rule.color = theme::hex_of(Color32::from_rgb(rgb[0], rgb[1], rgb[2]));
                    *persist = true;
                }
                if ui.small_button("Remove").clicked() {
                    remove = Some(i);
                }
                ui.end_row();
            }
        });
    if ui.button("Add rule").clicked() {
        add = true;
    }
    if let Some(i) = remove {
        app.settings.color_rules.remove(i);
        *persist = true;
    }
    if add {
        app.settings.color_rules.push(ColorRule {
            pattern: "*.log".into(),
            color: "8a8a8a".into(),
        });
        *persist = true;
    }
}

fn advanced_page(app: &mut ScApp, ui: &mut egui::Ui) {
    ui.heading("Advanced");
    ui.add_space(6.0);

    let dir = crate::config::config_dir();
    let settings_path = crate::config::settings_path();
    ui.label("Config folder");
    ui.monospace(dir.display().to_string());
    ui.label("Settings file");
    ui.monospace(settings_path.display().to_string());
    ui.add_space(8.0);
    if ui.button("Open config folder").clicked() {
        if let Err(e) = sc_shell::context::shell_open(&dir) {
            app.toast(e, true);
        }
    }
    ui.add_space(12.0);
    ui.weak("Settings are portable: they live next to the executable when that folder is writable, otherwise in %APPDATA%\\SimpleCommander.");
}
