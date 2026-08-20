//! Main window layout: toolbar, dual panes (tabs + virtualized file table),
//! status bar, keyboard handling, drag & drop.

use crate::app::{AddressEdit, ScApp, SearchMode, TabRename};
use crate::interact::{
    dest_label, drop_allowed, paint_drop_target, paint_insert_caret,
    paint_marquee, DropEffect, DropHint, FileDrag, ListPress, Marquee, TabDrag,
};
use crate::jobs::Job;
use crate::theme::LABEL_COLORS;
use egui::text::CCursorRange;
use egui::{Align2, Color32, CursorIcon, Key, Modifiers, Rect, RichText, Sense, TextEdit, Ui};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use egui_extras::{Column, TableBuilder};
use sc_core::entry::format_size;
use sc_core::sort::SortKey;
use sc_core::state::{PaneLayout, SplitDirection};

pub fn draw(app: &mut ScApp, ui: &mut Ui) {
    // Display-only: rebuilt by whichever drop target holds the pointer.
    app.drop_hint = None;
    let ctx = ui.ctx().clone();
    // Windows uses 500 ms; egui defaults to 300 ms, which makes deliberate
    // double-clicks on file rows register as two separate clicks.
    ctx.options_mut(|o| o.input_options.max_double_click_delay = 0.5);
    handle_global_keys(app, &ctx);
    top_bar(app, ui);
    crate::sidebar::draw(app, ui);
    if app.preview.enabled
        && app.settings.preview_placement == crate::config::PreviewPlacement::Right
    {
        let inner = egui::Panel::right("preview-dock")
            .resizable(true)
            .default_size(app.preview_width)
            .min_size(200.0)
            .max_size(800.0)
            .show(ui, |ui| {
                crate::preview::draw_docked_panel(app, ui);
            });
        let w = inner.response.rect.width();
        if (w - app.preview_width).abs() > 0.5 {
            app.preview_width = w.clamp(200.0, 800.0);
        }
    }
    status_bar(app, ui);
    ops_panel(app, ui);
    if app.preview.enabled
        && app.settings.preview_placement == crate::config::PreviewPlacement::Bottom
    {
        let inner = egui::Panel::bottom("preview-dock-bottom")
            .resizable(true)
            .default_size(app.preview_height)
            .min_size(140.0)
            .max_size(600.0)
            .show(ui, |ui| {
                crate::preview::draw_docked_panel(app, ui);
            });
        let h = inner.response.rect.height();
        if (h - app.preview_height).abs() > 0.5 {
            app.preview_height = h.clamp(140.0, 600.0);
        }
    }
    central_panes(app, ui);
    pane_background_menu(app, &ctx);
    row_context_menu_popup(app, &ctx);
    crate::dialogs::draw(app, &ctx);
    crate::settings_ui::draw(app, &ctx);
    crate::preview::draw(app, &ctx);
    search_overlay(app, &ctx);
    palette_overlay(app, &ctx);
    toasts(app, &ctx);
    crate::interact::draw_drag_badge(&ctx, &app.theme, app.drop_hint.as_ref());
    handle_file_drops(app, &ctx);
}

// ---------------------------------------------------------------- keyboard

fn text_edit_focused(ctx: &egui::Context) -> bool {
    ctx.text_edit_focused()
}

static PREV_CTRL_C: AtomicBool = AtomicBool::new(false);
static PREV_CTRL_X: AtomicBool = AtomicBool::new(false);
static PREV_CTRL_V: AtomicBool = AtomicBool::new(false);

fn async_key_down(vk: i32) -> bool {
    unsafe { windows::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState(vk) < 0 }
}

/// Rising edge of Ctrl+letter. Needed because file-only clipboards produce
/// no egui Paste event, and Copy/Cut events can be dropped the same way.
fn async_ctrl_letter_edge(vk: i32, prev: &AtomicBool) -> bool {
    let down = async_key_down(0x11) && async_key_down(vk);
    let was = prev.swap(down, Ordering::Relaxed);
    down && !was
}

fn surrender_text_focus(ctx: &egui::Context) {
    if let Some(id) = ctx.memory(|m| m.focused()) {
        ctx.memory_mut(|m| m.surrender_focus(id));
    }
}

/// Focus a text field and select its contents — the usual “about to edit” behavior.
pub(crate) fn select_all_on_focus(
    ui: &mut Ui,
    output: &mut egui::text_edit::TextEditOutput,
    take_focus: &mut bool,
) {
    let want = *take_focus || output.response.gained_focus();
    if *take_focus {
        output.response.request_focus();
        *take_focus = false;
    }
    if want {
        output
            .state
            .cursor
            .set_char_range(Some(CCursorRange::select_all(&output.galley)));
        let id = output.response.response.id;
        output.state.clone().store(ui.ctx(), id);
    }
}

fn handle_global_keys(app: &mut ScApp, ctx: &egui::Context) {
    if app.capture_shortcut.is_some() {
        match crate::keymap::take_binding(ctx) {
            crate::keymap::Capture::Cancel => app.capture_shortcut = None,
            crate::keymap::Capture::Bound(ch) => {
                if let Some(id) = app.capture_shortcut.take() {
                    *app.settings.keymap.get_mut(id) = ch;
                    app.persist_settings();
                }
            }
            crate::keymap::Capture::Wait => {}
        }
        return;
    }

    if ctx.input(|i| i.key_pressed(Key::Escape)) {
        let closed_edit = app.rename.take().is_some()
            || app.address_edit.take().is_some()
            || app.tab_rename.take().is_some();
        let closed_win = if closed_edit {
            false
        } else {
            app.dismiss_floating_ui()
        };
        if closed_edit || closed_win {
            ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape));
        }
    }

    let km = app.settings.keymap.clone();
    let typing = text_edit_focused(ctx);

    // Track Ctrl+C/X/V even while a text field is focused so releasing
    // the keys there does not fire a file copy/paste on the next frame.
    let edge_c = async_ctrl_letter_edge(0x43, &PREV_CTRL_C);
    let edge_x = async_ctrl_letter_edge(0x58, &PREV_CTRL_X);
    let edge_v = async_ctrl_letter_edge(0x56, &PREV_CTRL_V);

    // Shortcuts that work even while typing (window-level).
    if km.palette.consume(ctx) {
        app.palette.open = !app.palette.open;
        app.palette.focus_requested = true;
        if app.palette.open {
            sc_shell::everything::warmup();
            app.run_palette();
        }
    }
    if km.settings.consume(ctx) {
        app.show_settings = !app.show_settings;
    }
    if km.search.consume(ctx) {
        app.open_search();
    }
    if km.filter.consume(ctx) {
        app.search.open = false;
        app.filter_focus = Some(app.active_pane);
    }
    if typing || app.new_item.is_some() || app.palette.open || app.search.open {
        return;
    }

    let pane = app.active_pane;
    if km.edit_address.consume(ctx) {
        app.address_edit = Some(AddressEdit {
            pane,
            buffer: app.panes[pane].tab().path.display().to_string(),
            focus_requested: true,
        });
    }
    if km.new_tab.consume(ctx) {
        let path = app.panes[pane].tab().path.clone();
        app.panes[pane].add_tab(path);
        let ti = app.panes[pane].active_tab;
        app.request_listing_for(pane, ti, false);
    }
    if km.close_tab.consume(ctx) {
        let ti = app.panes[pane].active_tab;
        app.panes[pane].close_tab(ti);
    }
    if km.refresh.consume(ctx) || ctx.input_mut(|i| i.consume_key(Modifiers::CTRL, Key::R)) {
        app.request_listing(pane, true);
    }
    if km.switch_pane.consume(ctx) {
        app.active_pane = app.other_pane(pane);
    }
    if km.new_folder.consume(ctx) {
        app.begin_new_folder(pane);
    }
    if km.open_terminal.consume(ctx) {
        app.open_terminal(pane);
    }
    if (km.go_up.consume(ctx)
        || km.parent_folder.consume(ctx)
        || ctx.input_mut(|i| i.consume_key(Modifiers::ALT, Key::ArrowUp)))
        && !sc_shell::recycle::is_recycle_path(&app.panes[pane].tab().path)
    {
        app.go_parent(pane);
    }
    if km.history_back.consume(ctx) {
        app.history_back(pane);
    }
    if km.history_forward.consume(ctx) {
        app.history_forward(pane);
    }
    if km.select_all.consume(ctx) {
        let tab = app.panes[pane].tab_mut();
        tab.selection = tab.view.iter().copied().collect();
    }
    // egui-winit converts Ctrl+C/X/V into Copy/Cut/Paste events and does not
    // emit Key events. File-only clipboards often produce no Paste event at all.
    let saw_copy = crate::keymap::take_copy_event(ctx);
    let saw_cut = crate::keymap::take_cut_event(ctx);
    let saw_paste = crate::keymap::take_paste_event(ctx);
    let mods = ctx.input(|i| i.modifiers);
    if ((saw_copy || edge_c) && km.copy_paths.matches_modifiers(mods))
        || km.copy_paths.consume(ctx)
    {
        app.copy_paths_to_clipboard(pane);
    } else if ((saw_copy || edge_c) && km.copy_to_other.matches_modifiers(mods))
        || km.copy_to_other.consume(ctx)
    {
        app.transfer_to_other_pane(pane, false);
    } else if saw_copy || edge_c || km.copy.consume(ctx) {
        app.copy_selection_to_clipboard(pane, false);
    }
    if saw_cut || edge_x || km.cut.consume(ctx) {
        app.copy_selection_to_clipboard(pane, true);
    }
    if saw_paste || edge_v || km.paste.consume(ctx) {
        app.paste_into(pane);
    }
    if km.move_to_other.consume(ctx) {
        app.transfer_to_other_pane(pane, true);
    }
    if km.undo.consume(ctx) {
        app.undo();
    }
    let redo_alias = ctx.input(|i| {
        i.modifiers.ctrl && i.modifiers.shift && !i.modifiers.alt && i.key_pressed(Key::Z)
    });
    if km.redo.consume(ctx) || redo_alias {
        app.redo();
    }
    if km.compare_folders.consume(ctx) {
        app.open_compare();
    }
    if km.rename.consume(ctx) {
        app.start_rename(pane);
    }
    if km.delete_permanent.consume(ctx) {
        app.delete_selection(pane, true);
    } else if km.delete.consume(ctx) {
        app.delete_selection(pane, false);
    }
    if km.toggle_hidden.consume(ctx) {
        app.show_hidden = !app.show_hidden;
        app.rebuild_view(0);
        app.rebuild_view(1);
    }
    if !app.search.open && !app.palette.open && km.toggle_preview.consume(ctx) {
        app.preview.enabled = !app.preview.enabled;
        if app.preview.enabled {
            app.preview.space_armed = false;
            app.preview.prev_space_down = true;
            update_preview_from_selection(app, pane);
        } else {
            crate::preview::close(&mut app.preview);
        }
    }
    if km.enter_folder.consume(ctx) {
        let open = {
            let tab = app.panes[pane].tab();
            tab.cursor.and_then(|pos| tab.view.get(pos).copied()).and_then(|ei| {
                let e = tab.snapshot.entries.get(ei as usize)?;
                let zip = !e.is_dir()
                    && e.name
                        .rsplit('.')
                        .next()
                        .map(|x| x.eq_ignore_ascii_case("zip"))
                        .unwrap_or(false);
                (e.is_dir() || zip).then_some(ei)
            })
        };
        if let Some(ei) = open {
            app.open_entry(pane, ei);
        }
    }

    // List navigation for the active pane.
    list_navigation_keys(app, ctx, pane);
    type_ahead(app, ctx, pane);
}

fn list_navigation_keys(app: &mut ScApp, ctx: &egui::Context, pane: usize) {
    let shift = ctx.input(|i| i.modifiers.shift);
    let mut moved: Option<isize> = None;
    if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::ArrowDown))
        || (shift && ctx.input_mut(|i| i.consume_key(Modifiers::SHIFT, Key::ArrowDown)))
    {
        moved = Some(1);
    }
    if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::ArrowUp))
        || (shift && ctx.input_mut(|i| i.consume_key(Modifiers::SHIFT, Key::ArrowUp)))
    {
        moved = Some(-1);
    }
    if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::PageDown))
        || (shift && ctx.input_mut(|i| i.consume_key(Modifiers::SHIFT, Key::PageDown)))
    {
        moved = Some(20);
    }
    if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::PageUp))
        || (shift && ctx.input_mut(|i| i.consume_key(Modifiers::SHIFT, Key::PageUp)))
    {
        moved = Some(-20);
    }
    let home = ctx.input_mut(|i| {
        i.consume_key(Modifiers::NONE, Key::Home) || i.consume_key(Modifiers::SHIFT, Key::Home)
    });
    let end = ctx.input_mut(|i| {
        i.consume_key(Modifiers::NONE, Key::End) || i.consume_key(Modifiers::SHIFT, Key::End)
    });

    let tab = app.panes[pane].tab_mut();
    if tab.view.is_empty() {
        return;
    }
    let max = tab.view.len() - 1;
    let mut new_pos = None;
    if home {
        new_pos = Some(0);
    } else if end {
        new_pos = Some(max);
    } else if let Some(delta) = moved {
        let cur = tab.cursor.unwrap_or(0) as isize;
        new_pos = Some((cur + delta).clamp(0, max as isize) as usize);
    }
    if let Some(pos) = new_pos {
        tab.move_cursor(pos, shift);
        app.force_scroll_tab = Some(tab.uid);
        update_preview_from_selection(app, pane);
    }

    if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Enter)) {
        let tab = app.panes[pane].tab();
        if let Some(pos) = tab.cursor {
            if let Some(&entry) = tab.view.get(pos) {
                app.open_entry(pane, entry);
            }
        }
    }
}

fn type_ahead(app: &mut ScApp, ctx: &egui::Context, pane: usize) {
    if ctx.input(|i| i.modifiers.ctrl || i.modifiers.command || i.modifiers.alt) {
        return;
    }
    let texts: Vec<String> = ctx.input(|i| {
        i.events
            .iter()
            .filter_map(|e| match e {
                egui::Event::Text(t) => Some(t.clone()),
                _ => None,
            })
            .collect()
    });
    if texts.is_empty() {
        return;
    }
    let incoming = texts.concat();
    if incoming.trim().is_empty() {
        return;
    }
    let timed_out =
        app.type_ahead_at.elapsed().as_millis() as u64 > app.settings.type_ahead_ms.max(100);
    if timed_out {
        app.type_ahead.clear();
    }
    app.type_ahead_at = std::time::Instant::now();

    let incoming_lower = incoming.to_lowercase();
    let same_letter = incoming_lower.chars().count() == 1
        && !app.type_ahead.is_empty()
        && app.type_ahead.to_lowercase() == incoming_lower;
    let cycle = same_letter && !timed_out;
    if !cycle {
        app.type_ahead.push_str(&incoming);
    }
    let needle = app.type_ahead.to_lowercase();
    let tab = app.panes[pane].tab_mut();
    if tab.view.is_empty() {
        return;
    }
    let n = tab.view.len();
    let start = if cycle {
        tab.cursor.map(|c| (c + 1) % n).unwrap_or(0)
    } else {
        0
    };
    let pos = (0..n).find_map(|off| {
        let i = (start + off) % n;
        let e = tab.snapshot.entries.get(tab.view[i] as usize)?;
        e.name.to_lowercase().starts_with(&needle).then_some(i)
    });
    if let Some(pos) = pos {
        let entry = tab.view[pos];
        tab.selection.clear();
        tab.selection.insert(entry);
        tab.cursor = Some(pos);
        tab.anchor = Some(pos);
        app.force_scroll_tab = Some(tab.uid);
    }
}

fn update_preview_from_selection(app: &mut ScApp, pane: usize) {
    if !app.preview.enabled {
        return;
    }
    let tab = app.panes[pane].tab();
    let file = tab.selection.iter().find_map(|&i| {
        let e = tab.snapshot.entries.get(i as usize)?;
        (!e.is_dir()).then(|| tab.path.join(&e.name))
    });
    app.request_preview(file);
}

// ---------------------------------------------------------------- top bar

fn top_bar(app: &mut ScApp, ui: &mut Ui) {
    let ctx = ui.ctx().clone();
    egui::Panel::top("topbar").show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.menu_button("File", |ui| {
                if ui
                    .button(format!("New folder\t{}", app.settings.keymap.new_folder.label()))
                    .clicked()
                {
                    app.begin_new_folder(app.active_pane);
                    ui.close();
                }
                if ui.button("New file").clicked() {
                    app.begin_new_file(app.active_pane);
                    ui.close();
                }
                ui.separator();
                if ui.button("Exit").clicked() {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            });
            ui.menu_button("Edit", |ui| {
                let pane = app.active_pane;
                let km = app.settings.keymap.clone();
                if ui.button(format!("Copy\t{}", km.copy.label())).clicked() {
                    app.copy_selection_to_clipboard(pane, false);
                    ui.close();
                }
                if ui.button(format!("Cut\t{}", km.cut.label())).clicked() {
                    app.copy_selection_to_clipboard(pane, true);
                    ui.close();
                }
                if ui.button(format!("Paste\t{}", km.paste.label())).clicked() {
                    app.paste_into(pane);
                    ui.close();
                }
                if ui.button(format!("Copy path(s)\t{}", km.copy_paths.label())).clicked() {
                    app.copy_paths_to_clipboard(pane);
                    ui.close();
                }
                if ui
                    .button(format!("Copy to other pane\t{}", km.copy_to_other.label()))
                    .clicked()
                {
                    app.transfer_to_other_pane(pane, false);
                    ui.close();
                }
                ui.separator();
                let undo_label = app.undo.undo_label().unwrap_or_else(|| "Undo".into());
                let undo_chord = app.settings.keymap.undo.label();
                if ui
                    .add_enabled(
                        app.undo.can_undo(),
                        egui::Button::new(format!("{undo_label}\t{undo_chord}")),
                    )
                    .clicked()
                {
                    app.undo();
                    ui.close();
                }
                let redo_label = app.undo.redo_label().unwrap_or_else(|| "Redo".into());
                let redo_chord = app.settings.keymap.redo.label();
                if ui
                    .add_enabled(
                        app.undo.can_redo(),
                        egui::Button::new(format!("{redo_label}\t{redo_chord}")),
                    )
                    .clicked()
                {
                    app.redo();
                    ui.close();
                }
                ui.separator();
                if ui.button("Batch rename...").clicked() {
                    open_batch_rename(app);
                    ui.close();
                }
            });
            ui.menu_button("View", |ui| {
                if ui.button("Toggle single/dual pane").clicked() {
                    app.toggle_layout();
                    ui.close();
                }
                if ui.button("Toggle split direction").clicked() {
                    app.layout = match app.layout {
                        PaneLayout::Dual(SplitDirection::Vertical) => {
                            PaneLayout::Dual(SplitDirection::Horizontal)
                        }
                        PaneLayout::Dual(SplitDirection::Horizontal) => {
                            PaneLayout::Dual(SplitDirection::Vertical)
                        }
                        s => s,
                    };
                    ui.close();
                }
                ui.separator();
                let mut show_hidden = app.show_hidden;
                if ui
                    .checkbox(
                        &mut show_hidden,
                        format!("Show hidden files\t{}", app.settings.keymap.toggle_hidden.label()),
                    )
                    .changed()
                {
                    app.show_hidden = show_hidden;
                    app.rebuild_view(0);
                    app.rebuild_view(1);
                }
                let mut preview = app.preview.enabled;
                if ui
                    .checkbox(
                        &mut preview,
                        format!("Preview\t{}", app.settings.keymap.toggle_preview.label()),
                    )
                    .changed()
                {
                    app.preview.enabled = preview;
                    if preview {
                        app.preview.space_armed = true;
                        update_preview_from_selection(app, app.active_pane);
                    } else {
                        crate::preview::close(&mut app.preview);
                    }
                }
                let mut toolbar = app.settings.show_toolbar;
                if ui.checkbox(&mut toolbar, "Toolbar").changed() {
                    app.settings.show_toolbar = toolbar;
                    app.persist_settings();
                }
                ui.label("Preview placement");
                let place = app.settings.preview_placement;
                for opt in [
                    crate::config::PreviewPlacement::Floating,
                    crate::config::PreviewPlacement::Right,
                    crate::config::PreviewPlacement::Bottom,
                ] {
                    if ui.radio(place == opt, opt.label()).clicked() {
                        app.settings.preview_placement = opt;
                        app.persist_settings();
                    }
                }
                let dual = matches!(app.layout, PaneLayout::Dual(_));
                if ui
                    .add_enabled(
                        dual,
                        egui::Button::new(format!(
                            "Compare folders\t{}",
                            app.settings.keymap.compare_folders.label()
                        )),
                    )
                    .clicked()
                {
                    app.open_compare();
                    ui.close();
                }
                if ui.button("Columns...").clicked() {
                    app.show_columns = true;
                    ui.close();
                }
                let pane = app.active_pane;
                let mut flatten = app.panes[pane].tab().flatten;
                if ui.checkbox(&mut flatten, "Flatten branch view").changed() {
                    app.panes[pane].tab_mut().flatten = flatten;
                    app.request_listing(pane, false);
                }
                ui.separator();
                ui.label("Theme:");
                for (id, label) in crate::theme::catalog() {
                    let selected = if *id == "amoled" {
                        crate::theme::is_amoled(app.theme.name)
                    } else {
                        app.theme.name == *id
                    };
                    if ui.radio(selected, *label).clicked() {
                        app.set_theme(id);
                        crate::theme::apply(&ctx, &app.theme);
                    }
                }
                ui.label(if crate::theme::is_amoled(app.theme.name) {
                    "AMOLED color:"
                } else {
                    "Accent:"
                });
                let mut hex = app.settings.accent.clone();
                if crate::theme::accent_editor(ui, &mut hex, app.theme.accent) {
                    app.set_accent(&hex);
                    crate::theme::apply(&ctx, &app.theme);
                    app.persist_settings();
                }
            });
            ui.menu_button("Go", |ui| {
                let pane = app.active_pane;
                let km = app.settings.keymap.clone();
                if ui.button(format!("Back\t{}", km.history_back.label())).clicked() {
                    app.history_back(pane);
                    ui.close();
                }
                if ui.button(format!("Forward\t{}", km.history_forward.label())).clicked() {
                    app.history_forward(pane);
                    ui.close();
                }
                if ui.button(format!("Up\t{}", km.go_up.label())).clicked() {
                    if !sc_shell::recycle::is_recycle_path(&app.panes[pane].tab().path) {
                        app.go_parent(pane);
                    }
                    ui.close();
                }
                if ui.button("Recycle Bin").clicked() {
                    app.navigate(pane, sc_shell::recycle::recycle_root());
                    ui.close();
                }
                ui.separator();
                if ui.button(format!("Quick jump...\t{}", km.palette.label())).clicked() {
                    app.palette.open = true;
                    app.palette.focus_requested = true;
                    sc_shell::everything::warmup();
                    app.run_palette();
                    ui.close();
                }
            });
            ui.menu_button("Tools", |ui| {
                if ui
                    .button(format!("Filter\t{}", app.settings.keymap.filter.label()))
                    .clicked()
                {
                    app.search.open = false;
                    app.filter_focus = Some(app.active_pane);
                    ui.close();
                }
                if ui
                    .button(format!("Search...\t{}", app.settings.keymap.search.label()))
                    .clicked()
                {
                    app.open_search();
                    ui.close();
                }
                if ui
                    .button(format!(
                        "Open terminal\t{}",
                        app.settings.keymap.open_terminal.label()
                    ))
                    .clicked()
                {
                    app.open_terminal(app.active_pane);
                    ui.close();
                }
                if ui
                    .button(format!(
                        "Compare folders\t{}",
                        app.settings.keymap.compare_folders.label()
                    ))
                    .clicked()
                {
                    app.open_compare();
                    ui.close();
                }
                if ui.button("Plugin manager...").clicked() {
                    app.show_plugin_manager = true;
                    ui.close();
                }
                if ui
                    .button(format!("Settings...\t{}", app.settings.keymap.settings.label()))
                    .clicked()
                {
                    app.show_settings = true;
                    ui.close();
                }
                ui.separator();
                // Command plugins.
                let commands: Vec<(usize, String)> = {
                    let host = app.engine.plugins.read();
                    let pane = app.active_pane;
                    let names = app.panes[pane].tab().selected_names();
                    let tab = app.panes[pane].tab();
                    let ext = names
                        .first()
                        .and_then(|n| tab.snapshot.entries.iter().find(|e| e.name == *n))
                        .map(|e| e.ext().to_ascii_lowercase())
                        .unwrap_or_default();
                    host.plugins
                        .iter()
                        .enumerate()
                        .filter(|(_, p)| p.is_command() && (ext.is_empty() || p.handles_ext(&ext)))
                        .map(|(i, p)| {
                            (
                                i,
                                if p.manifest.command_label.is_empty() {
                                    p.manifest.name.clone()
                                } else {
                                    p.manifest.command_label.clone()
                                },
                            )
                        })
                        .collect()
                };
                if commands.is_empty() {
                    ui.weak("No command plugins installed");
                }
                for (idx, label) in commands {
                    if ui.button(&label).clicked() {
                        run_plugin_command(app, idx, &label);
                        ui.close();
                    }
                }
            });
            ui.menu_button("Help", |ui| {
                if ui.button("About SimpleCommander").clicked() {
                    app.show_about = true;
                    ui.close();
                }
            });

            ui.separator();
            // Layout quick toggles.
            let vertical = matches!(app.layout, PaneLayout::Dual(SplitDirection::Vertical));
            let horizontal = matches!(app.layout, PaneLayout::Dual(SplitDirection::Horizontal));
            let single = matches!(app.layout, PaneLayout::Single);
            if crate::icons::button(ui, crate::icons::Glyph::DualVertical, vertical, "Dual vertical").clicked() {
                app.layout = PaneLayout::Dual(SplitDirection::Vertical);
            }
            if crate::icons::button(ui, crate::icons::Glyph::DualHorizontal, horizontal, "Dual horizontal").clicked()
            {
                app.layout = PaneLayout::Dual(SplitDirection::Horizontal);
            }
            if crate::icons::button(ui, crate::icons::Glyph::Single, single, "Single pane").clicked() {
                app.layout = PaneLayout::Single;
            }
            ui.separator();
            if crate::icons::button(
                ui,
                crate::icons::Glyph::Terminal,
                false,
                &format!("Open terminal ({})", app.settings.keymap.open_terminal.label()),
            )
            .clicked()
            {
                app.open_terminal(app.active_pane);
            }
            if crate::icons::button(
                ui,
                crate::icons::Glyph::Gear,
                false,
                &format!("Settings ({})", app.settings.keymap.settings.label()),
            )
            .clicked()
            {
                app.show_settings = true;
            }
        });
        if app.settings.show_toolbar {
            ui.painter().hline(
                ui.max_rect().x_range(),
                ui.cursor().min.y - 1.0,
                egui::Stroke::new(1.0, app.theme.separator),
            );
            ui.add_space(2.0);
            action_toolbar(app, ui);
            ui.add_space(1.0);
        }
    });
}

/// Height of the action toolbar row, matching `icons::button`'s 20pt buttons.
const TOOLBAR_ROW_H: f32 = 20.0;

/// One toolbar action, dispatched after the row is drawn so the render loop
/// never holds a borrow of `app`.
#[derive(Clone, Copy)]
enum ToolbarAction {
    NewFolder,
    NewFile,
    Cut,
    Copy,
    Paste,
    CopyPath,
    Rename,
    Delete,
    CopyToOther,
    MoveToOther,
    Compare,
    Refresh,
    ToggleFavorite,
    ToggleHidden,
    Search,
}

enum ToolbarItem {
    Separator,
    Button {
        glyph: crate::icons::Glyph,
        tip: String,
        enabled: bool,
        selected: bool,
        action: ToolbarAction,
    },
}

/// Row of one-click actions for the active pane, right-aligned.
///
/// The row is built as data rather than as inline widget calls because the
/// right-to-left layout needs the list walked backwards (see below); declaring
/// the buttons once in reading order keeps that detail in one place.
///
/// Buttons that need a selection, or a non-empty clipboard, are disabled rather
/// than hidden: the row keeps the same shape so its layout stays learnable.
fn action_toolbar(app: &mut ScApp, ui: &mut Ui) {
    use crate::icons::Glyph;
    use ToolbarAction as A;

    let pane = app.active_pane;
    let (has_sel, dir_path, in_recycle) = {
        let tab = app.panes[pane].tab();
        (
            !tab.selection.is_empty(),
            tab.path.clone(),
            sc_shell::recycle::is_recycle_path(&tab.path),
        )
    };
    let writable = !in_recycle && crate::vfs::split_zip_path(&dir_path).is_none();
    let can_paste = sc_shell::clipboard::clipboard_has_files() && writable;
    let is_fav = app.is_favorite(&dir_path);
    let dual = matches!(app.layout, PaneLayout::Dual(_));
    let k = &app.settings.keymap;

    let button = |glyph, enabled, selected, tip: String, action| ToolbarItem::Button {
        glyph,
        tip,
        enabled,
        selected,
        action,
    };
    let items = vec![
        button(Glyph::NewFolder, writable, false, "New folder".into(), A::NewFolder),
        button(Glyph::NewFile, writable, false, "New file".into(), A::NewFile),
        ToolbarItem::Separator,
        button(
            Glyph::Cut,
            has_sel && writable,
            false,
            format!("Cut ({})", k.cut.label()),
            A::Cut,
        ),
        button(
            Glyph::Copy,
            has_sel,
            false,
            format!("Copy ({})", k.copy.label()),
            A::Copy,
        ),
        button(
            Glyph::Paste,
            can_paste,
            false,
            format!("Paste ({})", k.paste.label()),
            A::Paste,
        ),
        button(
            Glyph::Link,
            true,
            false,
            "Copy path of the selection, or of this folder".into(),
            A::CopyPath,
        ),
        ToolbarItem::Separator,
        button(
            Glyph::Rename,
            has_sel && writable,
            false,
            format!("Rename ({})", k.rename.label()),
            A::Rename,
        ),
        button(
            Glyph::Trash,
            has_sel,
            false,
            format!("Delete ({})", k.delete.label()),
            A::Delete,
        ),
        ToolbarItem::Separator,
        button(
            Glyph::Copy,
            has_sel && dual,
            false,
            format!("Copy to other pane ({})", k.copy_to_other.label()),
            A::CopyToOther,
        ),
        button(
            Glyph::Move,
            has_sel && dual,
            false,
            format!("Move to other pane ({})", k.move_to_other.label()),
            A::MoveToOther,
        ),
        button(Glyph::Compare, dual, false, "Compare folders".into(), A::Compare),
        ToolbarItem::Separator,
        button(
            Glyph::Refresh,
            true,
            false,
            format!("Refresh ({})", k.refresh.label()),
            A::Refresh,
        ),
        button(
            if is_fav { Glyph::StarFilled } else { Glyph::Star },
            true,
            is_fav,
            if is_fav {
                "Remove this folder from favorites".into()
            } else {
                "Add this folder to favorites".into()
            },
            A::ToggleFavorite,
        ),
        button(
            Glyph::Eye,
            true,
            app.show_hidden,
            "Show hidden files".into(),
            A::ToggleHidden,
        ),
        button(
            Glyph::Search,
            true,
            app.search.open,
            format!("Search ({})", k.search.label()),
            A::Search,
        ),
    ];

    let mut fired: Option<ToolbarAction> = None;

    // Right-aligned: `right_to_left` places the first widget added furthest
    // right, so the list is walked backwards to keep the buttons reading in
    // their declared order.
    //
    // The height is allocated explicitly. `with_layout` would hand the child
    // the whole available rect, which inside a top panel is the full window
    // height — the panel then grows to fit it and swallows the window.
    let row = egui::vec2(ui.available_width(), TOOLBAR_ROW_H);
    ui.allocate_ui_with_layout(row, egui::Layout::right_to_left(egui::Align::Center), |ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        for item in items.iter().rev() {
            match item {
                ToolbarItem::Separator => {
                    ui.add_space(2.0);
                    ui.separator();
                    ui.add_space(2.0);
                }
                ToolbarItem::Button {
                    glyph,
                    tip,
                    enabled,
                    selected,
                    action,
                } => {
                    if crate::icons::button_enabled(ui, *glyph, *selected, *enabled, tip).clicked()
                    {
                        fired = Some(*action);
                    }
                }
            }
        }
    });

    let Some(action) = fired else { return };
    match action {
        A::NewFolder => app.begin_new_folder(pane),
        A::NewFile => app.begin_new_file(pane),
        A::Cut => app.copy_selection_to_clipboard(pane, true),
        A::Copy => app.copy_selection_to_clipboard(pane, false),
        A::Paste => app.paste_into(pane),
        A::CopyPath => app.copy_paths_to_clipboard(pane),
        A::Rename => app.start_rename(pane),
        A::Delete => app.delete_selection(pane, false),
        A::CopyToOther => app.transfer_to_other_pane(pane, false),
        A::MoveToOther => app.transfer_to_other_pane(pane, true),
        A::Compare => app.open_compare(),
        A::Refresh => app.request_listing(pane, true),
        A::ToggleFavorite => app.toggle_favorite(dir_path),
        A::ToggleHidden => {
            app.show_hidden = !app.show_hidden;
            app.settings.session.show_hidden = app.show_hidden;
            app.rebuild_view(0);
            app.rebuild_view(1);
            app.persist_settings();
        }
        A::Search => app.open_search(),
    }
}

pub fn run_plugin_command(app: &mut ScApp, plugin_index: usize, label: &str) {
    let paths: Vec<String> = app.panes[app.active_pane]
        .tab()
        .selected_paths()
        .iter()
        .map(|p| p.display().to_string())
        .collect();
    let result = app.engine.plugins.read().run_command(plugin_index, &paths);
    match result {
        Ok(output) => app.plugin_output = Some((label.to_string(), output)),
        Err(e) => app.toast(format!("Plugin error: {e}"), true),
    }
}

pub fn open_batch_rename(app: &mut ScApp) {
    let pane = app.active_pane;
    let items = app.panes[pane].tab().selected_paths();
    if items.is_empty() {
        app.toast("Select files to batch rename".into(), false);
        return;
    }
    app.batch_rename.open = true;
    app.batch_rename.pane = pane;
    app.batch_rename.items = items;
    app.batch_rename.error = None;
}

// ---------------------------------------------------------------- panes

fn central_panes(app: &mut ScApp, ui: &mut Ui) {
    app.pane_rects = vec![egui::Rect::NOTHING; 2];
    egui::CentralPanel::default().show(ui, |ui| match app.layout {
        PaneLayout::Single => pane_view(app, ui, app.active_pane.min(1)),
        PaneLayout::Dual(dir) => split_panes(app, ui, dir),
    });
}

/// Ratio-based splitter for the dual layout: both panes get exact rects and a
/// draggable divider in between. (Nested resizable panels proved unreliable
/// here: the first panel could swallow the full area, hiding the second pane.)
fn split_panes(app: &mut ScApp, ui: &mut Ui, dir: SplitDirection) {
    const HANDLE: f32 = 6.0;
    const MIN_RATIO: f32 = 0.15;
    const MAX_RATIO: f32 = 0.85;

    let full = ui.available_rect_before_wrap();
    if full.width() < HANDLE + 2.0 || full.height() < HANDLE + 2.0 {
        return;
    }

    // Interact with the handle first (using last frame's ratio for its
    // position) so the drag updates the ratio before we lay out the panes.
    let handle_id = ui.id().with("split-handle");
    let handle_rect_for = |ratio: f32| -> egui::Rect {
        match dir {
            SplitDirection::Vertical => {
                let w0 = ((full.width() - HANDLE) * ratio).round();
                egui::Rect::from_min_size(
                    egui::pos2(full.min.x + w0, full.min.y),
                    egui::vec2(HANDLE, full.height()),
                )
            }
            SplitDirection::Horizontal => {
                let h0 = ((full.height() - HANDLE) * ratio).round();
                egui::Rect::from_min_size(
                    egui::pos2(full.min.x, full.min.y + h0),
                    egui::vec2(full.width(), HANDLE),
                )
            }
        }
    };
    let resp = ui.interact(handle_rect_for(app.split_ratio), handle_id, Sense::drag());
    if resp.dragged() {
        if let Some(pos) = resp.interact_pointer_pos() {
            let frac = match dir {
                SplitDirection::Vertical => (pos.x - full.min.x) / full.width().max(1.0),
                SplitDirection::Horizontal => (pos.y - full.min.y) / full.height().max(1.0),
            };
            app.split_ratio = frac.clamp(MIN_RATIO, MAX_RATIO);
        }
    }
    if resp.double_clicked() {
        app.split_ratio = 0.5;
    }
    if resp.hovered() || resp.dragged() {
        ui.output_mut(|o| {
            o.cursor_icon = match dir {
                SplitDirection::Vertical => egui::CursorIcon::ResizeHorizontal,
                SplitDirection::Horizontal => egui::CursorIcon::ResizeVertical,
            }
        });
    }

    let ratio = app.split_ratio.clamp(MIN_RATIO, MAX_RATIO);
    let handle_rect = handle_rect_for(ratio);
    let (rect0, rect1) = match dir {
        SplitDirection::Vertical => (
            egui::Rect::from_min_max(full.min, egui::pos2(handle_rect.min.x, full.max.y)),
            egui::Rect::from_min_max(egui::pos2(handle_rect.max.x, full.min.y), full.max),
        ),
        SplitDirection::Horizontal => (
            egui::Rect::from_min_max(full.min, egui::pos2(full.max.x, handle_rect.min.y)),
            egui::Rect::from_min_max(egui::pos2(full.min.x, handle_rect.max.y), full.max),
        ),
    };

    // Divider line.
    let stroke = if resp.hovered() || resp.dragged() {
        egui::Stroke::new(2.0, app.theme.accent_dim)
    } else {
        egui::Stroke::new(1.0, app.theme.separator)
    };
    let c = handle_rect.center();
    match dir {
        SplitDirection::Vertical => ui.painter().line_segment(
            [egui::pos2(c.x, handle_rect.min.y), egui::pos2(c.x, handle_rect.max.y)],
            stroke,
        ),
        SplitDirection::Horizontal => ui.painter().line_segment(
            [egui::pos2(handle_rect.min.x, c.y), egui::pos2(handle_rect.max.x, c.y)],
            stroke,
        ),
    };

    for (pane, rect) in [(0usize, rect0), (1usize, rect1)] {
        let builder = egui::UiBuilder::new()
            .max_rect(rect)
            .layout(egui::Layout::top_down(egui::Align::Min))
            .id_salt(("split-pane", pane));
        ui.scope_builder(builder, |ui| {
            ui.set_clip_rect(rect.intersect(ui.clip_rect()));
            ui.set_min_size(rect.size());
            ui.set_max_size(rect.size());
            pane_view(app, ui, pane);
        });
    }
}

fn pane_view(app: &mut ScApp, ui: &mut Ui, pane: usize) {
    let rect = ui.available_rect_before_wrap();
    if pane < app.pane_rects.len() {
        app.pane_rects[pane] = rect;
    }
    // Activate pane on any click inside it.
    if ui.input(|i| i.pointer.any_pressed())
        && ui.rect_contains_pointer(rect)
    {
        app.active_pane = pane;
    }

    tab_bar(app, ui, pane);
    // Hairline under the tab strip: on AMOLED the toolbar, tabs, list and
    // status bar are all pure black and would otherwise bleed into one another.
    let after_tabs = ui.cursor().min.y;
    ui.painter().hline(
        rect.x_range(),
        after_tabs - 1.0,
        egui::Stroke::new(1.0, app.theme.separator),
    );
    address_bar(app, ui, pane);
    file_table(app, ui, pane);

    // A rail on the leading edge marks the pane that has focus, so a dual-pane
    // layout is never ambiguous about where keys will land.
    if app.active_pane == pane && matches!(app.layout, PaneLayout::Dual(_)) {
        ui.painter().rect_filled(
            Rect::from_min_size(rect.min, egui::vec2(2.0, rect.height())),
            0.0,
            app.theme.accent,
        );
    }
}

/// A file drop target: paints feedback while hovered, and reports the payload
/// on release.
///
/// `resp` must come from the widget that actually occupies the target area, so
/// egui's `contains_pointer` (which is occlusion-aware) decides whether this
/// target is really under the pointer. `dnd_release_payload` consumes the
/// payload, so only the top-most target can accept a given drop and callers
/// never have to rank candidates.
fn file_drop_target(
    app: &mut ScApp,
    resp: &egui::Response,
    rect: Rect,
    dest: &Path,
) -> Option<(Vec<PathBuf>, usize)> {
    let drag = resp.dnd_hover_payload::<FileDrag>()?;
    let allowed = drop_allowed(&drag.paths, dest);
    paint_drop_target(
        &resp.ctx.layer_painter(resp.layer_id),
        &resp.ctx,
        rect,
        &app.theme,
        allowed,
    );
    app.drop_hint = Some(DropHint {
        dest: dest_label(dest),
        allowed,
    });
    if !allowed {
        // Still consume the drop so it does not fall through to a target
        // underneath that would happily accept it.
        if resp.dnd_release_payload::<FileDrag>().is_some() {
            app.toast(
                "Nothing to do here (same folder, or a folder into itself)".into(),
                false,
            );
        }
        return None;
    }
    resp.dnd_release_payload::<FileDrag>()
        .map(|d| (d.paths.clone(), d.from_pane))
}

fn tab_bar(app: &mut ScApp, ui: &mut Ui, pane: usize) {
    let active = app.active_pane == pane;
    let accent = app.theme.accent;
    let hover_bg = app.theme.hover_bg;
    let selection_bg = app.theme.selection_bg;
    let separator = app.theme.separator;
    let mut switch_to: Option<usize> = None;
    let mut close: Option<usize> = None;
    let mut toggle_lock: Option<usize> = None;
    let mut duplicate: Option<usize> = None;
    let mut start_rename: Option<usize> = None;
    let mut set_color: Option<(usize, Option<String>)> = None;
    // (dragged tab uid, insertion index in this pane).
    let mut drop_at: Option<(u64, usize)> = None;
    let mut file_drop: Option<(Vec<PathBuf>, PathBuf, usize)> = None;
    let mut tab_rects: Vec<(usize, Rect)> = Vec::new();
    let mut rename_commit = false;
    let mut rename_cancel = false;
    let mut need_dir_icon = false;

    egui::ScrollArea::horizontal()
        .id_salt(("tabs", pane))
        .auto_shrink([false, true])
        .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
        .show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            ui.set_height(26.0);
            let tab_count = app.panes[pane].tabs.len();
            let can_close_any = tab_count > 1;
            for i in 0..tab_count {
                let uid = app.panes[pane].tabs[i].uid;
                let locked = app.panes[pane].tabs[i].locked;
                let is_active_tab = i == app.panes[pane].active_tab;
                let title = app.panes[pane].tabs[i].title();
                let tab_hex = app.panes[pane].tabs[i].color.clone();
                let tab_color = tab_hex.as_deref().and_then(crate::theme::parse_hex);
                let renaming = app
                    .tab_rename
                    .as_ref()
                    .map(|r| r.pane == pane && r.index == i)
                    .unwrap_or(false);

                let mut fill = if is_active_tab { selection_bg } else { hover_bg };
                if let Some(c) = tab_color {
                    fill = Color32::from_rgb(
                        ((fill.r() as u16 * 2 + c.r() as u16) / 3) as u8,
                        ((fill.g() as u16 * 2 + c.g() as u16) / 3) as u8,
                        ((fill.b() as u16 * 2 + c.b() as u16) / 3) as u8,
                    );
                }
                let stroke_c = if is_active_tab { accent } else { separator };

                let mut close_rect: Option<Rect> = None;
                let inner = egui::Frame::new()
                    .inner_margin(egui::Margin::symmetric(6, 3))
                    .corner_radius(4)
                    .fill(fill)
                    .stroke(egui::Stroke::new(1.0, stroke_c))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 4.0;
                            ui.set_min_height(20.0);
                            if renaming {
                                if let Some(r) = &mut app.tab_rename {
                                    let mut output = TextEdit::singleline(&mut r.buffer)
                                        .desired_width(120.0)
                                        .show(ui);
                                    select_all_on_focus(ui, &mut output, &mut r.focus_requested);
                                    if output.response.lost_focus() {
                                        if ui.input(|inp| inp.key_pressed(Key::Enter)) {
                                            rename_commit = true;
                                        } else {
                                            rename_cancel = true;
                                        }
                                    }
                                    if ui.input(|inp| inp.key_pressed(Key::Escape)) {
                                        rename_cancel = true;
                                    }
                                }
                            } else {
                                match app.icons.map.get("dir") {
                                    Some(Some(tex)) => {
                                        ui.add(egui::Image::new(egui::load::SizedTexture::new(
                                            tex.id(),
                                            egui::vec2(16.0, 16.0),
                                        )));
                                    }
                                    Some(None) => {
                                        ui.label("📁");
                                    }
                                    None => {
                                        need_dir_icon = true;
                                        ui.label("📁");
                                    }
                                }
                                let text = if is_active_tab && active {
                                    RichText::new(&title).color(accent)
                                } else if is_active_tab {
                                    RichText::new(&title).strong()
                                } else {
                                    RichText::new(&title)
                                };
                                ui.add(egui::Label::new(text).selectable(false));
                            }
                            if can_close_any && !locked {
                                // Only reserve the space here. The interact is
                                // registered after the tab body below, so egui
                                // gives the × priority over the tab itself
                                // instead of us re-testing its rect by hand.
                                let (rect, _) = ui.allocate_exact_size(
                                    egui::vec2(14.0, 14.0),
                                    Sense::hover(),
                                );
                                close_rect = Some(rect);
                            }
                        });
                    });
                let r = inner.response.rect;
                tab_rects.push((i, r));
                if !renaming {
                    let tab_resp = ui
                        .interact(
                            r,
                            ui.id().with(("tab", pane, uid)),
                            Sense::click_and_drag(),
                        )
                        .on_hover_cursor(CursorIcon::Default);
                    // Registered after the tab body, so it wins the hit test
                    // wherever the two overlap.
                    let close_resp = close_rect.map(|cr| {
                        ui.interact(cr, ui.id().with(("tab-close", pane, uid)), Sense::click())
                            .on_hover_cursor(CursorIcon::Default)
                            .on_hover_text("Close tab")
                    });
                    if let Some(cr) = &close_resp {
                        if cr.clicked() {
                            close = Some(i);
                        }
                        let tint = if cr.contains_pointer() {
                            app.theme.error
                        } else {
                            app.theme.text_weak
                        };
                        crate::icons::paint_glyph(
                            ui.painter(),
                            cr.rect,
                            crate::icons::Glyph::Close,
                            tint,
                        );
                    }

                    let dest = app.panes[pane].tabs[i].path.clone();
                    if let Some((paths, from_pane)) =
                        file_drop_target(app, &tab_resp, r, &dest)
                    {
                        file_drop = Some((paths, dest.clone(), from_pane));
                    }

                    // Tab reorder. The insertion side is decided by which half
                    // of this tab the pointer is over, the same way egui's own
                    // drag-and-drop demo resolves an insertion index.
                    if let Some(d) = tab_resp.dnd_hover_payload::<TabDrag>() {
                        if d.uid != uid {
                            let before = ui
                                .ctx()
                                .pointer_interact_pos()
                                .is_some_and(|p| p.x < r.center().x);
                            let x = if before { r.left() - 2.0 } else { r.right() + 2.0 };
                            paint_insert_caret(ui, x, r.y_range(), accent);
                        }
                    }
                    if let Some(d) = tab_resp.dnd_release_payload::<TabDrag>() {
                        let before = ui
                            .ctx()
                            .pointer_interact_pos()
                            .is_some_and(|p| p.x < r.center().x);
                        drop_at = Some((d.uid, if before { i } else { i + 1 }));
                    }

                    if !egui::DragAndDrop::has_any_payload(ui.ctx()) {
                        tab_resp.dnd_set_drag_payload(TabDrag { uid });
                        if tab_resp.clicked() {
                            switch_to = Some(i);
                        }
                        if tab_resp.middle_clicked() {
                            close = Some(i);
                        }
                    }
                    {
                        tab_resp.context_menu(|ui| {
                            if ui.button("Rename tab…").clicked() {
                                start_rename = Some(i);
                                ui.close();
                            }
                            if ui.button(if locked { "Unlock tab" } else { "Lock tab" }).clicked() {
                                toggle_lock = Some(i);
                                ui.close();
                            }
                            if ui.button("Duplicate tab").clicked() {
                                duplicate = Some(i);
                                ui.close();
                            }
                            ui.menu_button("Color", |ui| {
                                for (name, c) in LABEL_COLORS {
                                    if name == "None" {
                                        if ui.button("None").clicked() {
                                            set_color = Some((i, None));
                                            ui.close();
                                        }
                                        continue;
                                    }
                                    let label = RichText::new(format!("● {name}")).color(c);
                                    if ui.button(label).clicked() {
                                        set_color = Some((i, Some(crate::theme::hex_of(c))));
                                        ui.close();
                                    }
                                }
                            });
                            if can_close_any && !locked && ui.button("Close tab").clicked() {
                                close = Some(i);
                                ui.close();
                            }
                        });
                    }
                }
                if let Some(c) = tab_color {
                    ui.painter().rect_filled(
                        egui::Rect::from_min_size(r.min, egui::vec2(3.0, r.height())),
                        egui::CornerRadius::ZERO,
                        c,
                    );
                }
                if is_active_tab {
                    ui.painter().hline(
                        r.x_range(),
                        r.bottom() - 1.5,
                        egui::Stroke::new(2.0, accent),
                    );
                }
            }

            let plus = ui
                .add(egui::Button::new("+").small().frame(false))
                .on_hover_cursor(CursorIcon::Default)
                .on_hover_text("New tab (Ctrl+T)");
            // Trailing strip: dropping a tab past the last one appends it.
            let tail = Rect::from_min_max(
                egui::pos2(plus.rect.left(), plus.rect.top()),
                egui::pos2(ui.max_rect().right().max(plus.rect.right()), plus.rect.bottom()),
            );
            let tail_resp = ui.interact(
                tail,
                ui.id().with(("tab-strip-tail", pane)),
                Sense::hover(),
            );
            let n_tabs = app.panes[pane].tabs.len();
            if tail_resp.dnd_hover_payload::<TabDrag>().is_some() {
                paint_insert_caret(ui, plus.rect.left() - 2.0, plus.rect.y_range(), accent);
            }
            if let Some(d) = tail_resp.dnd_release_payload::<TabDrag>() {
                drop_at = Some((d.uid, n_tabs));
            }
            if plus.clicked() {
                let path = app.panes[pane].tab().path.clone();
                app.panes[pane].add_tab(path);
                let ti = app.panes[pane].active_tab;
                app.request_listing_for(pane, ti, false);
            }
        });
    });

    if need_dir_icon && !app.icons.pending.contains("dir") && !app.icons.map.contains_key("dir") {
        app.icons.pending.insert("dir".into());
        app.engine.submit(Job::IconExt {
            key: "dir".into(),
            ext: String::new(),
            is_dir: true,
        });
    }
    if let Some(i) = switch_to {
        app.panes[pane].active_tab = i;
        app.active_pane = pane;
    }
    if let Some(i) = toggle_lock {
        app.panes[pane].tabs[i].locked = !app.panes[pane].tabs[i].locked;
    }
    if let Some((i, color)) = set_color {
        if i < app.panes[pane].tabs.len() {
            app.panes[pane].tabs[i].color = color;
        }
    }
    if let Some(i) = duplicate {
        let path = app.panes[pane].tabs[i].path.clone();
        let title = app.panes[pane].tabs[i].custom_title.clone();
        let color = app.panes[pane].tabs[i].color.clone();
        app.panes[pane].add_tab(path);
        let ti = app.panes[pane].active_tab;
        app.panes[pane].tabs[ti].custom_title = title;
        app.panes[pane].tabs[ti].color = color;
        app.request_listing_for(pane, ti, false);
    }
    if let Some(i) = start_rename {
        let name = app.panes[pane].tabs[i]
            .custom_title
            .clone()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| app.panes[pane].tabs[i].folder_name());
        app.tab_rename = Some(TabRename {
            pane,
            index: i,
            buffer: name,
            focus_requested: true,
        });
    }
    if rename_commit {
        if let Some(r) = app.tab_rename.take() {
            if r.pane < app.panes.len() && r.index < app.panes[r.pane].tabs.len() {
                let name = r.buffer.trim();
                app.panes[r.pane].tabs[r.index].custom_title = if name.is_empty() {
                    None
                } else {
                    Some(name.to_string())
                };
            }
        }
    } else if rename_cancel {
        app.tab_rename = None;
    }
    if let Some((uid, to_index)) = drop_at {
        if let Some((from_pane, from_index)) = app.find_tab_by_uid(uid) {
            // `to_index` is an insertion index into the pane as it looks now;
            // `reorder_tab` already compensates for removing the source first.
            app.relocate_tab(from_pane, from_index, pane, to_index);
        }
    }
    if let Some((paths, dest, from_pane)) = file_drop {
        app.drop_files_into_from(paths, dest, DropEffect::current(ui.ctx()), from_pane);
    }
    if let Some(i) = close {
        app.panes[pane].close_tab(i);
    }
}

fn address_bar(app: &mut ScApp, ui: &mut Ui, pane: usize) {
    ui.horizontal(|ui| {
        ui.set_height(22.0);
        if crate::icons::button(ui, crate::icons::Glyph::Back, false, "Back (Alt+Left)").clicked()
        {
            app.history_back(pane);
        }
        if crate::icons::button(ui, crate::icons::Glyph::Forward, false, "Forward (Alt+Right)").clicked()
        {
            app.history_forward(pane);
        }
        if crate::icons::button(ui, crate::icons::Glyph::Up, false, "Up (Backspace)").clicked() {
            if !sc_shell::recycle::is_recycle_path(&app.panes[pane].tab().path) {
                app.go_parent(pane);
            }
        }
        if crate::icons::button(ui, crate::icons::Glyph::Refresh, false, "Refresh (F5)").clicked() {
            app.request_listing(pane, true);
        }

        let path = app.panes[pane].tab().path.display().to_string();
        let editing_this = app.address_edit.as_ref().map(|a| a.pane) == Some(pane);
        let filter_w = 130.0;
        let addr_w = (ui.available_width() - filter_w - 8.0).max(80.0);

        let mut commit = false;
        let mut cancel = false;
        if editing_this {
            if let Some(edit) = &mut app.address_edit {
                let mut output = TextEdit::singleline(&mut edit.buffer)
                    .id(egui::Id::new(("addr-bar", pane)))
                    .clip_text(true)
                    .desired_width(addr_w)
                    .show(ui);
                select_all_on_focus(ui, &mut output, &mut edit.focus_requested);
                if output.response.lost_focus() {
                    if ui.input(|i| i.key_pressed(Key::Enter)) {
                        commit = true;
                    } else {
                        cancel = true;
                    }
                }
                if ui.input(|i| i.key_pressed(Key::Escape)) {
                    cancel = true;
                }
            }
        } else {
            let mut nav: Option<PathBuf> = None;
            let mut new_tab: Option<PathBuf> = None;
            let mut start_edit = false;
            ui.allocate_ui_with_layout(
                egui::vec2(addr_w, 22.0),
                egui::Layout::left_to_right(egui::Align::Center),
                |ui| {
                    let parts = breadcrumb_parts(&app.panes[pane].tab().path);
                    ui.menu_button("▾", |ui| {
                        for v in &app.volumes {
                            if ui.button(v.root.display().to_string()).clicked() {
                                nav = Some(v.root.clone());
                                ui.close();
                            }
                        }
                        if ui.button("Recycle Bin").clicked() {
                            nav = Some(sc_shell::recycle::recycle_root());
                            ui.close();
                        }
                        for (name, path) in app.wsl_distros.clone() {
                            if ui.button(format!("WSL: {name}")).clicked() {
                                nav = Some(path);
                                ui.close();
                            }
                        }
                    });
                    for (i, (label, dest)) in parts.iter().enumerate() {
                        if i > 0 {
                            ui.weak("›");
                        }
                        let r = ui.add(egui::Button::new(label).frame(false));
                        if r.clicked() {
                            nav = Some(dest.clone());
                        }
                        if r.middle_clicked() {
                            new_tab = Some(dest.clone());
                        }
                    }
                    let fill = ui.available_width().max(8.0);
                    let r = ui.allocate_response(egui::vec2(fill, 20.0), Sense::click());
                    if r.clicked() {
                        start_edit = true;
                    }
                    r.on_hover_text("Click or Ctrl+L to edit path");
                },
            );
            if start_edit {
                app.address_edit = Some(AddressEdit {
                    pane,
                    buffer: path.clone(),
                    focus_requested: true,
                });
            }
            if let Some(p) = new_tab {
                app.open_folder_in_new_tab(pane, p);
            } else if let Some(p) = nav {
                app.navigate(pane, p);
            }
        }

        if commit {
            if let Some(edit) = app.address_edit.take() {
                let dest = PathBuf::from(edit.buffer.trim());
                if sc_shell::recycle::is_recycle_path(&dest) {
                    app.navigate(pane, sc_shell::recycle::recycle_root());
                } else if dest.is_dir() || crate::vfs::zip_listing(&dest).is_some() {
                    app.navigate(pane, dest);
                } else {
                    app.toast("Path not found".into(), true);
                }
            }
        } else if cancel {
            app.address_edit = None;
        }

        let mut filter = app.panes[pane].tab().filter.clone();
        let mut output = TextEdit::singleline(&mut filter)
            .hint_text(format!("Filter * ({})", app.settings.keymap.filter.label()))
            .clip_text(true)
            .desired_width(filter_w)
            .show(ui);
        if app.filter_focus == Some(pane) {
            let mut take_filter = true;
            app.filter_focus = None;
            select_all_on_focus(ui, &mut output, &mut take_filter);
        }
        if output.response.changed() {
            app.panes[pane].tab_mut().filter = filter;
            app.rebuild_view(pane);
        }
    });
    ui.separator();
}

#[derive(Clone)]
enum ColKind {
    Index,
    Name,
    Size,
    Type,
    Modified,
    Created,
    Checksum,
    Plugin(usize, String),
}

fn visible_columns(app: &ScApp, plugin_columns: &[(usize, String)]) -> Vec<ColKind> {
    let mut out = Vec::new();
    let mut used_plugins: Vec<usize> = Vec::new();
    for pref in &app.settings.columns {
        if pref.id == "name" {
            out.push(ColKind::Name);
            continue;
        }
        if !pref.visible {
            continue;
        }
        match pref.id.as_str() {
            "index" => out.push(ColKind::Index),
            "size" => out.push(ColKind::Size),
            "type" => out.push(ColKind::Type),
            "modified" => out.push(ColKind::Modified),
            "created" => out.push(ColKind::Created),
            "sha256" => out.push(ColKind::Checksum),
            id if id.starts_with("plugin:") => {
                let rest = &id["plugin:".len()..];
                if let Some((pi, title)) = plugin_columns.iter().find(|(_, t)| t == rest) {
                    used_plugins.push(*pi);
                    out.push(ColKind::Plugin(*pi, title.clone()));
                }
            }
            _ => {}
        }
    }
    if !out.iter().any(|c| matches!(c, ColKind::Name)) {
        out.insert(0, ColKind::Name);
    }
    if let Some(i) = out.iter().position(|c| matches!(c, ColKind::Index)) {
        if i != 0 {
            let index = out.remove(i);
            out.insert(0, index);
        }
    }
    for (pi, title) in plugin_columns {
        if !used_plugins.contains(pi)
            && !app
                .settings
                .columns
                .iter()
                .any(|c| c.id == format!("plugin:{title}") && !c.visible)
        {
            out.push(ColKind::Plugin(*pi, title.clone()));
        }
    }
    out
}

fn column_label(id: &str) -> String {
    match id {
        "index" => "#".to_string(),
        "name" => "Name".to_string(),
        "size" => "Size".to_string(),
        "type" => "Type".to_string(),
        "modified" => "Modified".to_string(),
        "created" => "Created".to_string(),
        "sha256" => "SHA-256".to_string(),
        id if id.starts_with("plugin:") => id["plugin:".len()..].to_string(),
        other => other.to_string(),
    }
}

/// Entries for the header right-click column picker: (id, label, visible), in
/// the user's saved order, plus any plugin columns that don't have a pref yet
/// (shown as visible, matching `visible_columns`' default-on fallback).
fn column_menu_entries(app: &ScApp, plugin_columns: &[(usize, String)]) -> Vec<(String, String, bool)> {
    let mut out: Vec<(String, String, bool)> = app
        .settings
        .columns
        .iter()
        .filter(|c| c.id != "name")
        .map(|c| (c.id.clone(), column_label(&c.id), c.visible))
        .collect();
    for (_, title) in plugin_columns {
        let id = format!("plugin:{title}");
        if !out.iter().any(|(existing, _, _)| *existing == id) {
            out.push((id, title.clone(), true));
        }
    }
    out
}

/// Right-click header menu: toggle column visibility inline, with a link to
/// the full Columns dialog for reordering / reset.
fn column_picker_menu(
    ui: &mut Ui,
    columns: &[(String, String, bool)],
    toggle: &mut Option<String>,
    open_columns: &mut bool,
) {
    ui.label(RichText::new("Columns").weak());
    for (id, label, visible) in columns {
        let mut v = *visible;
        if ui.checkbox(&mut v, label).changed() {
            *toggle = Some(id.clone());
        }
    }
    ui.separator();
    if ui.button("More columns…").clicked() {
        *open_columns = true;
        ui.close();
    }
}

/// Make a table cell's interact rect cover the whole cell, not just the label.
fn row_cell(row: &mut egui_extras::TableRow<'_, '_>, add: impl FnOnce(&mut Ui)) {
    row.col(|ui| {
        ui.expand_to_include_rect(ui.max_rect());
        add(ui);
        ui.expand_to_include_rect(ui.max_rect());
    });
}

fn col_spec(kind: &ColKind) -> Column {
    match kind {
        ColKind::Index => Column::initial(40.0).at_least(28.0).clip(true),
        ColKind::Name => Column::remainder().at_least(140.0).clip(true),
        ColKind::Size => Column::initial(84.0).at_least(60.0).clip(true),
        ColKind::Type => Column::initial(64.0).at_least(40.0).clip(true),
        ColKind::Modified | ColKind::Created => Column::initial(128.0).at_least(90.0).clip(true),
        ColKind::Checksum => Column::initial(148.0).at_least(80.0).clip(true),
        ColKind::Plugin(_, _) => Column::initial(100.0).at_least(60.0).clip(true),
    }
}

fn col_header(kind: &ColKind) -> (&'static str, Option<SortKey>) {
    match kind {
        ColKind::Index => ("#", None),
        ColKind::Name => ("Name", Some(SortKey::Name)),
        ColKind::Size => ("Size", Some(SortKey::Size)),
        ColKind::Type => ("Type", Some(SortKey::Type)),
        ColKind::Modified => ("Modified", Some(SortKey::Modified)),
        ColKind::Created => ("Created", Some(SortKey::Created)),
        ColKind::Checksum => ("SHA-256", None),
        ColKind::Plugin(_, _) => ("", None),
    }
}

struct RowAction {
    open: Option<u32>,
    press_row: Option<(u32, usize, bool)>,
    click_row: Option<(u32, usize)>,
    context_on: Option<u32>,
    open_new_tab: Option<PathBuf>,
    windows_menu: bool,
    row_menu_pos: Option<egui::Pos2>,
    /// Accepted in-app file drop: (sources, destination, source pane).
    file_drop: Option<(Vec<PathBuf>, PathBuf, usize)>,
}

/// Why a file list has nothing to show.
enum EmptyState {
    Loading,
    EmptyFolder,
    /// A filter is hiding everything; `hidden` is how many entries it hid.
    NoMatches { filter: String, hidden: usize },
    Error(String),
}

/// Centred message for an empty list. Returns true if the user asked to clear
/// the filter.
fn paint_empty_state(ui: &mut Ui, theme: &crate::theme::Theme, body: Rect, state: &EmptyState) -> bool {
    let (glyph, headline, detail) = match state {
        EmptyState::Loading => ("⋯", "Loading…".to_string(), None),
        EmptyState::EmptyFolder => ("▢", "This folder is empty".to_string(), None),
        EmptyState::NoMatches { filter, hidden } => (
            "▢",
            format!("No files match  {filter}"),
            Some(format!(
                "{hidden} {} hidden by the filter",
                if *hidden == 1 { "item" } else { "items" }
            )),
        ),
        EmptyState::Error(err) => ("!", "Cannot read this folder".to_string(), Some(err.clone())),
    };
    let is_error = matches!(state, EmptyState::Error(_));

    let mut clear = false;
    let center = body.center();
    let builder = egui::UiBuilder::new()
        .max_rect(Rect::from_center_size(
            egui::pos2(center.x, center.y.min(body.min.y + 180.0)),
            egui::vec2(body.width().min(420.0), 130.0),
        ))
        .layout(egui::Layout::top_down(egui::Align::Center));
    ui.scope_builder(builder, |ui| {
        ui.spacing_mut().item_spacing.y = 5.0;
        ui.add(
            egui::Label::new(
                RichText::new(glyph)
                    .size(26.0)
                    .color(if is_error { theme.error } else { theme.separator }),
            )
            .selectable(false),
        );
        ui.add(
            egui::Label::new(
                RichText::new(headline)
                    .size(13.0)
                    .color(if is_error { theme.error } else { theme.text_weak }),
            )
            .selectable(false),
        );
        if let Some(detail) = detail {
            ui.add(
                egui::Label::new(
                    RichText::new(detail).size(11.5).color(theme.text_weak.gamma_multiply(0.75)),
                )
                .selectable(false)
                .wrap(),
            );
        }
        if matches!(state, EmptyState::NoMatches { .. }) {
            ui.add_space(3.0);
            if ui
                .add(
                    egui::Label::new(
                        RichText::new("Clear filter").size(11.5).color(theme.accent).underline(),
                    )
                    .sense(Sense::click()),
                )
                .on_hover_cursor(CursorIcon::PointingHand)
                .clicked()
            {
                clear = true;
            }
        }
    });
    clear
}

fn file_table(app: &mut ScApp, ui: &mut Ui, pane: usize) {
    let theme = app.theme;
    // Gather column plugins applicable here.
    let plugin_columns: Vec<(usize, String)> = {
        let host = app.engine.plugins.read();
        host.plugins
            .iter()
            .enumerate()
            .filter(|(_, p)| p.is_column())
            .map(|(i, p)| {
                (
                    i,
                    if p.manifest.column_title.is_empty() {
                        p.manifest.name.clone()
                    } else {
                        p.manifest.column_title.clone()
                    },
                )
            })
            .collect()
    };

    let tab_uid = app.panes[pane].tab().uid;
    let dir_path = app.panes[pane].tab().path.clone();
    let dir_meta = app.tags.dir_meta(&dir_path).clone();
    let color_rules: Vec<(sc_core::sort::Wildcard, Color32)> = app
        .settings
        .color_rules
        .iter()
        .filter_map(|r| {
            let c = u32::from_str_radix(&r.color, 16).ok()?;
            Some((
                sc_core::sort::Wildcard::new(&r.pattern),
                Color32::from_rgb((c >> 16) as u8, (c >> 8) as u8, c as u8),
            ))
        })
        .collect();

    let mut action = RowAction {
        open: None,
        press_row: None,
        click_row: None,
        context_on: None,
        open_new_tab: None,
        windows_menu: false,
        row_menu_pos: None,
        file_drop: None,
    };
    let mut sort_click: Option<SortKey> = None;
    let mut rename_commit = false;
    let mut rename_cancel = false;
    let mut icon_requests: Vec<(String, String, bool, Option<PathBuf>)> = Vec::new();
    let mut column_requests: Vec<(usize, PathBuf)> = Vec::new();
    let mut checksum_requests: Vec<PathBuf> = Vec::new();

    let (entries, view, selection, cursor, tab_sort) = {
        let tab = app.panes[pane].tab();
        (
            tab.snapshot.entries.clone(),
            tab.view.clone(),
            tab.selection.clone(),
            tab.cursor,
            tab.sort,
        )
    };
    let row_height = app.settings.row_height.clamp(16.0, 36.0);
    let striped = app.settings.striped_rows;
    let show_icons = app.settings.show_icons;
    let single_click = app.settings.single_click_open;
    let cols = visible_columns(app, &plugin_columns);
    let mut open_columns = false;
    let mut toggle_column: Option<String> = None;
    let menu_columns = column_menu_entries(app, &plugin_columns);
    let force_scroll = app.force_scroll_tab == Some(tab_uid);
    let restore_scroll = app.panes[pane].tab().scroll_y;
    let wants_scroll = ctx_wants_scroll(ui)
        || app
            .rename
            .as_ref()
            .is_some_and(|r| r.tab_uid == tab_uid && r.focus_requested)
        || force_scroll;
    // A file list wants contiguous rows: the global 4pt vertical spacing makes
    // them read as detached cards and leaves the hover and selection bands
    // floating. Zeroing it here also makes the row pitch equal the row height.
    ui.spacing_mut().item_spacing.y = 0.0;
    // Selection is muted in the pane that does not have focus, so it is obvious
    // at a glance where the next keystroke lands. The table reads this from its
    // parent Ui's visuals, so it has to be set before the builder runs.
    let pane_active = app.active_pane == pane;
    if !pane_active {
        let idle = crate::theme::mix_rgb(theme.selection_bg, theme.bg, 0.55);
        ui.visuals_mut().selection.bg_fill = idle;
    }
    // The space left for the table, NOT `ui.max_rect()`: max_rect is the whole
    // pane, so deriving the body from it put `list_bg` on top of the tab strip
    // and address bar. Because `list_bg` is registered after those widgets, it
    // won the hit test and swallowed every breadcrumb and edit-path click.
    let table_rect = ui.available_rect_before_wrap();
    let header_h = 22.0;
    // Everything below the column headers. `Ui::interact` only registers a
    // widget rect, so this claims no layout space and does not move the table.
    let body_rect = Rect::from_min_max(
        egui::pos2(table_rect.min.x, table_rect.min.y + header_h + 2.0),
        table_rect.max,
    );
    // Registered BEFORE the rows on purpose: egui resolves overlapping widgets
    // in favour of the one added last, so each row wins its own strip and this
    // background is only hit in the gaps below/around them. That ordering is
    // what makes "click empty space to deselect" and "drop on the pane body"
    // work without any manual rect comparisons.
    let list_bg = ui.interact(
        body_rect,
        ui.id().with(("list-bg", pane, tab_uid)),
        Sense::click_and_drag(),
    );
    let mut row_got_secondary = false;
    // (view row, row top in screen space) for the rows actually laid out, used
    // to calibrate the rubber-band's geometry instead of assuming it.
    let mut row_tops: Vec<(usize, f32)> = Vec::new();
    let new_scroll_y = {
        let n = view.len();
        let avail_height = ui.available_height();
        let body_h = (avail_height - header_h).max(1.0);
        let mut table = TableBuilder::new(ui)
            .id_salt(("files", pane, tab_uid))
            .striped(striped)
            .resizable(true)
            .min_scrolled_height(body_h)
            .max_scroll_height(body_h)
        .auto_shrink([false, false])
        .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
        .sense(Sense::click_and_drag());
        let has_index = cols.iter().any(|c| matches!(c, ColKind::Index));
        if has_index {
            table = table.column(col_spec(&ColKind::Index));
        }
        if show_icons {
            table = table.column(Column::exact(20.0));
        }
        for col in cols.iter().filter(|c| !matches!(c, ColKind::Index)) {
            table = table.column(col_spec(col));
        }
        if let (Some(pos), true) = (cursor, wants_scroll) {
            table = table.scroll_to_row(pos, None);
        } else {
            table = table.vertical_scroll_offset(restore_scroll);
        }

        let scroll_out = table
            .header(22.0, |mut header| {
                if has_index {
                    let (_rect, resp) = header.col(|ui| {
                        ui.with_layout(
                            egui::Layout::right_to_left(egui::Align::Center),
                            |ui| {
                                ui.add(
                                    egui::Label::new(
                                        RichText::new("#").size(10.5).color(theme.text_weak),
                                    )
                                    .selectable(false),
                                );
                            },
                        );
                    });
                    resp.context_menu(|ui| {
                        column_picker_menu(ui, &menu_columns, &mut toggle_column, &mut open_columns);
                    });
                }
                if show_icons {
                    header.col(|_| {});
                }
                for col in cols.iter().filter(|c| !matches!(c, ColKind::Index)) {
                    let (label, key) = col_header(col);
                    let label = match col {
                        ColKind::Plugin(_, title) => title.as_str(),
                        _ => label,
                    };
                    let arrow = if let Some(k) = key {
                        if tab_sort.key == k {
                            if tab_sort.ascending { Some("▲") } else { Some("▼") }
                        } else {
                            None
                        }
                    } else {
                        None
                    };
                    let (_rect, resp) = header.col(|ui| {
                        ui.allocate_ui_with_layout(
                            ui.available_size(),
                            egui::Layout::left_to_right(egui::Align::Center),
                            |ui| {
                                ui.spacing_mut().item_spacing.x = 3.0;
                                // Headers label the data, they are not the data:
                                // small, quiet and uppercase keeps them from
                                // competing with the filenames below.
                                ui.add(
                                    egui::Label::new(
                                        RichText::new(label.to_uppercase())
                                            .size(10.5)
                                            .color(theme.text_weak),
                                    )
                                    .selectable(false),
                                );
                                if let Some(arrow) = arrow {
                                    ui.add(
                                        egui::Label::new(
                                            RichText::new(arrow).size(9.0).color(theme.accent),
                                        )
                                        .selectable(false),
                                    );
                                }
                            },
                        );
                    });
                    if key.is_some() && resp.clicked() {
                        sort_click = key;
                    }
                    resp.context_menu(|ui| {
                        column_picker_menu(ui, &menu_columns, &mut toggle_column, &mut open_columns);
                    });
                }
            })
            .body(|body| {
                body.rows(row_height, n, |mut row| {
                    let vpos = row.index();
                    let ei = view[vpos];
                    let entry = &entries[ei as usize];
                    let selected = selection.contains(&ei);
                    row.set_selected(selected);
                    if cursor == Some(vpos) {
                        row.set_hovered(true);
                    }

                    if has_index {
                    row_cell(&mut row, |ui| {
                        ui.with_layout(
                            egui::Layout::right_to_left(egui::Align::Center),
                            |ui| {
                                ui.add(
                                    egui::Label::new(
                                        RichText::new((vpos + 1).to_string()).weak(),
                                    )
                                    .selectable(false),
                                );
                            },
                        );
                    });
                    }

                    if show_icons {
                    row_cell(&mut row, |ui| {
                        let ext = entry.ext();
                        let per_file = sc_shell::icons::needs_per_file_icon(ext);
                        let key = if entry.is_dir() {
                            "dir".to_string()
                        } else if per_file {
                            format!("path:{}", dir_path.join(&entry.name).display())
                        } else {
                            let mut k = String::with_capacity(4 + ext.len());
                            k.push_str("ext:");
                            k.extend(ext.chars().map(|c| c.to_ascii_lowercase()));
                            k
                        };
                        match app.icons.map.get(&key) {
                            Some(Some(tex)) => {
                                let (rect, _) = ui.allocate_exact_size(
                                    egui::vec2(16.0, 16.0),
                                    Sense::hover(),
                                );
                                ui.painter().image(
                                    tex.id(),
                                    rect,
                                    egui::Rect::from_min_max(
                                        egui::pos2(0.0, 0.0),
                                        egui::pos2(1.0, 1.0),
                                    ),
                                    Color32::WHITE,
                                );
                            }
                            Some(None) => {
                                ui.label(if entry.is_dir() { "📁" } else { "📄" });
                            }
                            None => {
                                if icon_requests.len() < 8 && !app.icons.pending.contains(&key) {
                                    icon_requests.push((
                                        key,
                                        ext.to_ascii_lowercase(),
                                        entry.is_dir(),
                                        per_file.then(|| dir_path.join(&entry.name)),
                                    ));
                                }
                                ui.label(if entry.is_dir() { "📁" } else { "📄" });
                            }
                        }
                    });
                    }

                    for col in cols.iter().filter(|c| !matches!(c, ColKind::Index)) {
                    match col {
                    ColKind::Index => {}
                    ColKind::Name => {
                    row_cell(&mut row, |ui| {
                        let renaming = app
                            .rename
                            .as_ref()
                            .map(|r| r.tab_uid == tab_uid && r.entry_index == ei)
                            .unwrap_or(false);
                        if renaming {
                            if let Some(r) = &mut app.rename {
                                let mut output = TextEdit::singleline(&mut r.buffer)
                                    .desired_width(f32::INFINITY)
                                    .show(ui);
                                select_all_on_focus(ui, &mut output, &mut r.focus_requested);
                                if output.response.lost_focus() {
                                    if ui.input(|i| i.key_pressed(Key::Enter)) {
                                        rename_commit = true;
                                    } else {
                                        rename_cancel = true;
                                    }
                                }
                                if ui.input(|i| i.key_pressed(Key::Escape)) {
                                    rename_cancel = true;
                                }
                            }
                        } else {
                            let mut color = if entry.is_dir() {
                                theme.folder
                            } else {
                                theme.text
                            };
                            if let Some(meta) = dir_meta.get(&entry.name) {
                                if meta.label > 0 && (meta.label as usize) < LABEL_COLORS.len() {
                                    color = LABEL_COLORS[meta.label as usize].1;
                                }
                            } else {
                                for (rule, c) in &color_rules {
                                    if rule.matches(&entry.name) {
                                        color = *c;
                                        break;
                                    }
                                }
                            }
                            if entry.is_hidden() {
                                color = color.gamma_multiply(0.55);
                            }
                            // The focused row reads as focused through its name,
                            // now that it no longer draws an outline.
                            if cursor == Some(vpos) && pane_active {
                                color = crate::theme::mix_rgb(color, theme.text_strong, 0.45);
                            }
                            ui.add(
                                egui::Label::new(RichText::new(&entry.name).color(color))
                                    .selectable(false),
                            );
                        }
                    });
                    }
                    ColKind::Size => {
                    row_cell(&mut row, |ui| {
                        // Monospace so digits line up down the column instead of
                        // jittering with the proportional face.
                        if entry.is_dir() {
                            if let Some(sz) = app.folder_sizes.get(&dir_path.join(&entry.name)) {
                                ui.add(
                                    egui::Label::new(
                                        RichText::new(format_size(*sz)).weak().monospace().size(11.5),
                                    )
                                    .selectable(false),
                                );
                            }
                        } else {
                            ui.add(
                                egui::Label::new(
                                    RichText::new(format_size(entry.size)).monospace().size(11.5),
                                )
                                .selectable(false),
                            );
                        }
                    });
                    }
                    ColKind::Type => {
                    row_cell(&mut row, |ui| {
                        ui.add(egui::Label::new(RichText::new(if entry.is_dir() { "dir" } else { entry.ext() }).weak()).selectable(false));
                    });
                    }
                    ColKind::Modified => {
                    row_cell(&mut row, |ui| {
                        ui.add(
                            egui::Label::new(
                                RichText::new(format_time(entry.modified))
                                    .weak()
                                    .monospace()
                                    .size(11.5),
                            )
                            .selectable(false),
                        );
                    });
                    }
                    ColKind::Created => {
                    row_cell(&mut row, |ui| {
                        ui.add(
                            egui::Label::new(
                                RichText::new(format_time(entry.created))
                                    .weak()
                                    .monospace()
                                    .size(11.5),
                            )
                            .selectable(false),
                        );
                    });
                    }
                    ColKind::Checksum => {
                    row_cell(&mut row, |ui| {
                        if entry.is_dir() {
                            return;
                        }
                        let full = dir_path.join(&entry.name);
                        match app.checksums.get(&full) {
                            Some(Some(v)) => {
                                ui.add(
                                    egui::Label::new(RichText::new(v).weak().monospace().size(11.0))
                                        .selectable(false)
                                        .truncate(),
                                )
                                .on_hover_text(v.clone());
                            }
                            Some(None) => {
                                ui.weak("—");
                            }
                            None => {
                                if checksum_requests.len() < 4
                                    && !app.checksum_pending.contains(&full)
                                {
                                    checksum_requests.push(full);
                                }
                                ui.weak("…");
                            }
                        }
                    });
                    }
                    ColKind::Plugin(pi, _) => {
                        row_cell(&mut row, |ui| {
                            let full = dir_path.join(&entry.name);
                            match app.column_values.get(&(*pi, full.clone())) {
                                Some(Some(v)) => {
                                    ui.add(egui::Label::new(RichText::new(v).weak()).selectable(false));
                                }
                                Some(None) => {}
                                None => {
                                    if !entry.is_dir()
                                        && !app.column_pending.contains(&(*pi, full.clone()))
                                    {
                                        column_requests.push((*pi, full));
                                    }
                                }
                            }
                        });
                    }
                    }
                    }

                    // One response for the whole row: the union of its cells,
                    // which span the table because one column is a remainder.
                    // This is the single source of truth for what the pointer
                    // is on — no raw pointer polling, so a popup covering the
                    // row correctly suppresses its clicks.
                    //
                    // Note: never call `.interact()` on this; egui documents
                    // that as undefined behaviour for a unioned response.
                    let resp = row.response();
                    let row_rect = resp.rect;
                    let ctx = resp.ctx.clone();
                    row_tops.push((vpos, row_rect.min.y));
                    // Selected rows get an accent rail on the leading edge. The
                    // keyboard cursor is shown by brightening its name instead;
                    // the old full-row outline sat on top of the selection fill
                    // and the two rectangles fought each other.
                    if selected {
                        let rail = if pane_active {
                            theme.accent
                        } else {
                            crate::theme::mix_rgb(theme.accent, theme.bg, 0.62)
                        };
                        ctx.layer_painter(resp.layer_id).rect_filled(
                            Rect::from_min_size(
                                row_rect.min,
                                egui::vec2(3.0, row_rect.height()),
                            ),
                            0.0,
                            rail,
                        );
                    }

                    // Drop destination: a folder row swallows the drop, any
                    // other row passes it to the folder being listed.
                    let dest = if entry.is_dir() {
                        dir_path.join(&entry.name)
                    } else {
                        dir_path.clone()
                    };
                    if let Some((paths, from_pane)) =
                        file_drop_target(app, &resp, row_rect, &dest)
                    {
                        action.file_drop = Some((paths, dest, from_pane));
                    }

                    // Drag source. Explorer drags whatever is selected; if the
                    // press landed outside the selection, that row becomes the
                    // selection first (the old code simply refused to drag).
                    if resp.drag_started_by(egui::PointerButton::Primary)
                        && app.marquee.is_none()
                        && app.rename.is_none()
                        && !egui::DragAndDrop::has_any_payload(&ctx)
                    {
                        let drag_selection = if selected {
                            selection.clone()
                        } else {
                            std::iter::once(ei).collect()
                        };
                        let paths: Vec<PathBuf> = view
                            .iter()
                            .filter(|e| drag_selection.contains(e))
                            .filter_map(|&idx| {
                                entries.get(idx as usize).map(|e| dir_path.join(&e.name))
                            })
                            .collect();
                        if !paths.is_empty() {
                            if !selected {
                                action.press_row = Some((ei, vpos, false));
                            }
                            resp.dnd_set_drag_payload(FileDrag {
                                paths,
                                from_pane: pane,
                            });
                        }
                    }

                    if !egui::DragAndDrop::has_any_payload(&ctx) {
                        if ctx.input(|i| i.pointer.primary_pressed()) && resp.contains_pointer() {
                            action.press_row = Some((ei, vpos, selected));
                        }
                        if resp.double_clicked_by(egui::PointerButton::Primary) {
                            action.open = Some(ei);
                            action.click_row = None;
                        } else if resp.clicked_by(egui::PointerButton::Primary) {
                            action.click_row = Some((ei, vpos));
                        }
                        if resp.secondary_clicked() {
                            row_got_secondary = true;
                            action.context_on = Some(ei);
                            action.row_menu_pos = ctx.pointer_interact_pos();
                            if ctx.input(|i| i.modifiers.shift) {
                                action.windows_menu = true;
                            }
                        }
                        let is_zip = !entry.is_dir() && ext_is_zip(&entry.name);
                        if resp.middle_clicked() && (entry.is_dir() || is_zip) {
                            action.open_new_tab = Some(dir_path.join(&entry.name));
                        }
                    }
                });
            });
        scroll_out.state.offset.y
    };
    app.panes[pane].tab_mut().scroll_y = new_scroll_y;
    if open_columns {
        app.show_columns = true;
    }
    if let Some(id) = toggle_column {
        if let Some(pref) = app.settings.columns.iter_mut().find(|c| c.id == id) {
            pref.visible = !pref.visible;
        } else {
            app.settings.columns.push(crate::config::ColumnPref { id, visible: false });
        }
        app.persist_settings();
    }

    // Blank space reads as a hang, so say what is going on.
    if view.is_empty() {
        let (loading, has_filter, filter, hidden, error) = {
            let tab = app.panes[pane].tab();
            (
                tab.loading,
                !tab.filter.is_empty(),
                tab.filter.clone(),
                tab.snapshot.entries.len(),
                tab.snapshot.error.clone(),
            )
        };
        let state = match error {
            Some(err) => EmptyState::Error(err),
            None if loading => EmptyState::Loading,
            None if has_filter => EmptyState::NoMatches { filter, hidden },
            None => EmptyState::EmptyFolder,
        };
        if paint_empty_state(ui, &theme, body_rect, &state) {
            app.panes[pane].tab_mut().filter.clear();
            app.rebuild_view(pane);
        }
    }

    // ---- list background: drop target, deselect, rubber-band ----
    // `list_bg` was registered before the rows, so egui reports it as hit only
    // where no row covers the pointer. Everything below therefore needs no
    // "was it on a row?" bookkeeping.
    if list_bg.clicked() {
        surrender_text_focus(ui.ctx());
    }
    if let Some((paths, from_pane)) = file_drop_target(app, &list_bg, body_rect, &dir_path) {
        action.file_drop = Some((paths, dir_path.clone(), from_pane));
    }

    let mods = ui.input(|i| i.modifiers);
    let ctrl = mods.ctrl || mods.command;
    let shift = mods.shift;
    let shift_held = shift;
    let n = view.len();
    // Calibrated from this frame's rows: `egui_extras` steps rows by
    // `row_height + item_spacing.y`, and the origin depends on the header and
    // the scroll offset. Measuring beats assuming — an error in the pitch would
    // grow with drag distance and silently skip rows.
    let metrics = crate::interact::RowMetrics::from_rows(
        &row_tops,
        row_height + ui.spacing().item_spacing.y,
    );

    if list_bg.secondary_clicked() && !row_got_secondary {
        if shift {
            action.windows_menu = true;
            action.context_on = None;
        } else if let Some(pos) = ui.ctx().pointer_interact_pos() {
            app.pane_bg_menu = Some((pane, pos));
        }
    }

    let dragging_files = egui::DragAndDrop::has_payload_of_type::<FileDrag>(ui.ctx());

    // Press on empty space: recorded so a release here can clear the selection.
    if list_bg.contains_pointer()
        && ui.input(|i| i.pointer.primary_pressed())
        && app.rename.is_none()
        && !egui::DragAndDrop::has_any_payload(ui.ctx())
    {
        app.list_press = Some(ListPress { pane, tab_uid, ctrl, shift });
    }

    // Press that landed on a row, applied after the table released its borrow.
    if let Some((_ei, vpos, was_selected)) = action.press_row {
        app.list_press = Some(ListPress { pane, tab_uid, ctrl, shift });
        if shift {
            app.panes[pane].tab_mut().move_cursor(vpos, true);
            update_preview_from_selection(app, pane);
        } else if !ctrl && !was_selected {
            // Select on press, so a drag starting here carries this row.
            app.panes[pane].tab_mut().move_cursor(vpos, false);
            update_preview_from_selection(app, pane);
        } else if !ctrl {
            // Already selected: leave the whole selection intact so it stays
            // draggable, and collapse it only if this becomes a plain click.
            app.panes[pane].tab_mut().cursor = Some(vpos);
        }
    }

    // ---- rubber-band selection ----
    // Only starts on empty space; a drag that begins on a row is a file drag,
    // which is what Explorer does and removes the old eligibility guesswork.
    let scroll_y = app.panes[pane].tab().scroll_y;
    if app.marquee.is_none()
        && !dragging_files
        && app.rename.is_none()
        && list_bg.drag_started_by(egui::PointerButton::Primary)
    {
        if let Some(origin) = ui.input(|i| i.pointer.press_origin()) {
            let keep = if ctrl {
                app.panes[pane].tab().selection.clone()
            } else {
                HashSet::new()
            };
            app.marquee = Some(Marquee {
                pane,
                tab_uid,
                anchor_content_y: metrics.to_content(origin.y),
                anchor_x: origin.x,
                additive: ctrl,
                keep,
            });
        }
    }

    let mut marquee_ended = false;
    let mut marquee_scroll = false;
    let marquee_here = app
        .marquee
        .as_ref()
        .is_some_and(|m| m.pane == pane && m.tab_uid == tab_uid);
    if dragging_files && marquee_here {
        marquee_ended = true;
    } else if let Some(m) = app.marquee.clone().filter(|_| marquee_here) {
        let primary_down = ui.input(|i| i.pointer.primary_down());
        let pos = ui
            .input(|i| i.pointer.interact_pos().or(i.pointer.hover_pos()))
            .unwrap_or_else(|| egui::pos2(m.anchor_x, metrics.to_screen(m.anchor_content_y)));

        // Edge autoscroll. Nudging the stored offset is enough: the band lives
        // in content space, so it follows the scroll without extra bookkeeping.
        if primary_down {
            ui.ctx().request_repaint();
            const EDGE: f32 = 24.0;
            let dt = ui.input(|i| i.unstable_dt).clamp(0.0, 0.05);
            let speed = 14.0 * row_height;
            let delta = if pos.y < body_rect.min.y + EDGE {
                -speed * dt
            } else if pos.y > body_rect.max.y - EDGE {
                speed * dt
            } else {
                0.0
            };
            if delta != 0.0 {
                let max_scroll = (metrics.content_height(n) - body_rect.height()).max(0.0);
                let next = (scroll_y + delta).clamp(0.0, max_scroll);
                if (next - scroll_y).abs() > f32::EPSILON {
                    app.panes[pane].tab_mut().scroll_y = next;
                    marquee_scroll = true;
                }
            }
        }

        // `metrics.origin_y` already reflects this frame's scroll position, so
        // no scroll term is needed here; an autoscroll applied above lands on
        // the next frame's metrics.
        let rows = m.rows(metrics.to_content(pos.y), metrics, n);
        {
            let tab = app.panes[pane].tab_mut();
            match rows {
                Some((a, b)) => {
                    tab.select_view_range(a, b, if m.additive { Some(&m.keep) } else { None })
                }
                None if m.additive => tab.selection = m.keep.clone(),
                None => {
                    tab.selection.clear();
                    tab.cursor = None;
                    tab.anchor = None;
                }
            }
        }
        let anchor_screen_y = metrics.to_screen(m.anchor_content_y);
        let band =
            Rect::from_two_pos(egui::pos2(m.anchor_x, anchor_screen_y), pos).intersect(body_rect);
        if band.width() > 1.0 && band.height() > 1.0 {
            paint_marquee(ui, band, theme.accent);
        }
        if !primary_down {
            marquee_ended = true;
        }
    }
    if marquee_ended {
        app.marquee = None;
        app.list_press = None;
        update_preview_from_selection(app, pane);
    }

    // Click on empty space clears the selection.
    if list_bg.clicked_by(egui::PointerButton::Primary)
        && app.marquee.is_none()
        && !marquee_ended
        && !dragging_files
        && !ctrl
        && !shift
    {
        let tab = app.panes[pane].tab_mut();
        if !tab.selection.is_empty() {
            tab.selection.clear();
            tab.cursor = None;
            tab.anchor = None;
            update_preview_from_selection(app, pane);
        }
        app.list_press = None;
    }

    // ---- apply deferred actions (after the immutable borrow ends) ----
    if let Some((paths, dest, from_pane)) = action.file_drop.take() {
        app.drop_files_into_from(paths, dest, DropEffect::current(ui.ctx()), from_pane);
    }
    if let Some(key) = sort_click {
        app.sort_by(pane, key);
    }
    for (key, ext, is_dir, per_path) in icon_requests {
        app.icons.pending.insert(key.clone());
        match per_path {
            Some(p) => app.engine.submit(Job::IconPath { key, path: p }),
            None => app.engine.submit(Job::IconExt { key, ext, is_dir }),
        }
    }
    for (pi, path) in column_requests {
        app.column_pending.insert((pi, path.clone()));
        app.engine.submit(Job::ColumnValue { plugin: pi, path });
    }
    for path in checksum_requests {
        app.checksum_pending.insert(path.clone());
        app.engine.submit(Job::Checksum { path });
    }
    if rename_commit {
        app.commit_rename();
    } else if rename_cancel {
        app.rename = None;
    }

    // Click on a row. The modifiers recorded at press time win, so a slow
    // click whose modifier was let go early still does what the user started.
    if let Some((ei, vpos)) = action.click_row {
        let press = app
            .list_press
            .as_ref()
            .filter(|p| p.pane == pane && p.tab_uid == tab_uid);
        let use_ctrl = press.map(|p| p.ctrl).unwrap_or(ctrl);
        let use_shift = press.map(|p| p.shift).unwrap_or(shift);
        if use_ctrl {
            app.panes[pane].tab_mut().toggle_select(vpos);
        } else if use_shift {
            app.panes[pane].tab_mut().move_cursor(vpos, true);
        } else {
            app.panes[pane].tab_mut().move_cursor(vpos, false);
            if single_click {
                action.open = Some(ei);
            }
        }
        update_preview_from_selection(app, pane);
        app.list_press = None;
    }

    if let Some(ei) = action.context_on {
        let tab = app.panes[pane].tab_mut();
        if !tab.selection.contains(&ei) {
            tab.selection.clear();
            tab.selection.insert(ei);
            if let Some(vpos) = tab.view.iter().position(|&x| x == ei) {
                tab.cursor = Some(vpos);
                tab.anchor = Some(vpos);
            }
        }
    }
    if let Some(ei) = action.open {
        app.open_entry(pane, ei);
    }
    if let Some(path) = action.open_new_tab {
        app.open_folder_in_new_tab(pane, path);
    }
    if action.windows_menu {
        let paths = if action.context_on.is_some() {
            app.panes[pane].tab().selected_paths()
        } else {
            vec![app.panes[pane].tab().path.clone()]
        };
        invoke_windows_menu(ui.ctx(), &paths, shift_held);
    } else if let (Some(ei), Some(pos)) = (action.context_on, action.row_menu_pos) {
        app.pane_bg_menu = None;
        app.row_ctx_menu = Some((pane, ei, pos));
    }
    if force_scroll && !marquee_scroll {
        app.force_scroll_tab = None;
    }
    if !ui.input(|i| i.pointer.primary_down())
        && app.marquee.is_none()
        && app.list_press.as_ref().is_some_and(|p| p.pane == pane)
    {
        app.list_press = None;
    }
}

/// Scroll-to-cursor only right after keyboard navigation, not on mouse wheel.
fn ctx_wants_scroll(ui: &Ui) -> bool {
    ui.ctx().input(|i| {
        i.key_pressed(Key::ArrowDown)
            || i.key_pressed(Key::ArrowUp)
            || i.key_pressed(Key::PageDown)
            || i.key_pressed(Key::PageUp)
            || i.key_pressed(Key::Home)
            || i.key_pressed(Key::End)
    })
}

#[inline]
fn ext_is_zip(name: &str) -> bool {
    name.rsplit('.').next().is_some_and(|x| x.eq_ignore_ascii_case("zip"))
}

fn row_context_menu_popup(app: &mut ScApp, ctx: &egui::Context) {
    let Some((pane, ei, pos)) = app.row_ctx_menu else {
        return;
    };
    let mut close = false;
    let area = egui::Area::new(egui::Id::new("row-ctx-menu"))
        .order(egui::Order::Foreground)
        .fixed_pos(pos)
        .constrain(true)
        .show(ctx, |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.set_min_width(200.0);
                if row_context_menu(app, ui, pane, ei) {
                    close = true;
                }
            });
        });
    if close {
        app.row_ctx_menu = None;
    } else if ctx.input(|i| i.pointer.primary_clicked() || i.key_pressed(Key::Escape))
        && !area.response.contains_pointer()
    {
        app.row_ctx_menu = None;
    }
}

fn row_context_menu(app: &mut ScApp, ui: &mut Ui, pane: usize, entry_index: u32) -> bool {
    let tab = app.panes[pane].tab();
    let dir = tab.path.clone();
    let Some(entry) = tab.snapshot.entries.get(entry_index as usize) else {
        return false;
    };
    let full = dir.join(&entry.name);
    if sc_shell::recycle::is_recycle_path(&dir) {
        if ui.button("Restore").clicked() {
            app.restore_recycle_names(&[entry.name.clone()]);
            return true;
        }
        if ui.button("Delete permanently").clicked() {
            app.delete_recycle_names(&[entry.name.clone()]);
            return true;
        }
        if let Some(item) = app.recycle_meta.get(&entry.name) {
            if let Some(orig) = &item.original_path {
                ui.weak(orig.display().to_string());
            }
        }
        return false;
    }
    let in_zip = crate::vfs::split_zip_path(&dir).is_some()
        || crate::vfs::split_zip_path(&full).map(|(z, i)| !i.is_empty() && z != full).unwrap_or(false);

    if ui.button("Open").clicked() {
        app.open_entry(pane, entry_index);
        return true;
    }
    if !in_zip {
        if ui.button("Open with...").clicked() {
            let _ = sc_shell::context::shell_open_with(&full);
            return true;
        }
        let fav_path = if entry.is_dir() {
            full.clone()
        } else {
            dir.clone()
        };
        let fav_label = if app.is_favorite(&fav_path) {
            "Remove from favorites"
        } else {
            "Add to favorites"
        };
        if ui.button(fav_label).clicked() {
            app.toggle_favorite(fav_path);
            return true;
        }
    }
    ui.separator();
    if in_zip {
        if ui.button("Extract to other pane").clicked() {
            app.transfer_to_other_pane(pane, false);
            return true;
        }
    } else {
        if ui.button("Copy to other pane").clicked() {
            app.transfer_to_other_pane(pane, false);
            return true;
        }
        if ui.button("Move to other pane").clicked() {
            app.transfer_to_other_pane(pane, true);
            return true;
        }
        ui.separator();
        if ui.button("Cut").clicked() {
            app.copy_selection_to_clipboard(pane, true);
            return true;
        }
        if ui.button("Copy").clicked() {
            app.copy_selection_to_clipboard(pane, false);
            return true;
        }
        if sc_shell::clipboard::clipboard_has_files() && ui.button("Paste").clicked() {
            app.paste_into(pane);
            return true;
        }
        if ui.button("Copy path").clicked() {
            let _ = sc_shell::clipboard::set_clipboard_text(&full.display().to_string());
            return true;
        }
        ui.separator();
        if ui
            .button(format!("Rename\t{}", app.settings.keymap.rename.label()))
            .clicked()
        {
            app.start_rename(pane);
            return true;
        }
        if ui.button("Batch rename...").clicked() {
            open_batch_rename(app);
            return true;
        }
        if ui
            .button(format!(
                "Delete (recycle)\t{}",
                app.settings.keymap.delete.label()
            ))
            .clicked()
        {
            app.delete_selection(pane, false);
            return true;
        }
        if ui
            .button(format!(
                "Delete permanently\t{}",
                app.settings.keymap.delete_permanent.label()
            ))
            .clicked()
        {
            app.delete_selection(pane, true);
            return true;
        }
        ui.separator();
        let mut close = false;
        ui.menu_button("Label", |ui| {
            for (i, (name, color)) in LABEL_COLORS.iter().enumerate() {
                let text = if i == 0 {
                    RichText::new(*name)
                } else {
                    RichText::new(format!("● {name}")).color(*color)
                };
                if ui.button(text).clicked() {
                    let paths = app.panes[pane].tab().selected_paths();
                    for p in paths {
                        app.tags.set_label(&p, i as u8);
                    }
                    close = true;
                    ui.close();
                }
            }
        });
        if close {
            return true;
        }
        if ui.button("Tags & comment...").clicked() {
            let meta = app.tags.get(&full);
            app.tag_edit = Some(crate::app::TagEditState {
                open: true,
                path: full.clone(),
                tags: meta.tags,
                comment: meta.comment,
            });
            return true;
        }
        ui.separator();
        if entry.is_dir() && ui.button("Calculate folder size").clicked() {
            app.request_dir_size(full.clone());
            return true;
        }
        if ui.button("Open in other pane").clicked() {
            let other = app.other_pane(pane);
            let target = if entry.is_dir() { full.clone() } else { dir.clone() };
            app.navigate(other, target);
            return true;
        }
        ui.separator();
        let commands: Vec<(usize, String)> = {
            let ext = entry.ext().to_ascii_lowercase();
            let host = app.engine.plugins.read();
            host.plugins
                .iter()
                .enumerate()
                .filter(|(_, p)| p.is_command() && p.handles_ext(&ext))
                .map(|(i, p)| {
                    (
                        i,
                        if p.manifest.command_label.is_empty() {
                            p.manifest.name.clone()
                        } else {
                            p.manifest.command_label.clone()
                        },
                    )
                })
                .collect()
        };
        for (idx, label) in commands {
            if ui.button(&label).clicked() {
                run_plugin_command(app, idx, &label);
                return true;
            }
        }
        ui.separator();
        if ui.button("Windows menu...").clicked() {
            let paths = app.panes[pane].tab().selected_paths();
            invoke_windows_menu(ui.ctx(), &paths, false);
            return true;
        }
        if ui.button("Properties\tAlt+Enter").clicked() {
            let _ = sc_shell::context::shell_properties(&full);
            return true;
        }
    }
    false
}

fn invoke_windows_menu(ctx: &egui::Context, paths: &[PathBuf], extended: bool) {
    if paths.is_empty() {
        return;
    }
    let pos = ctx.input(|i| {
        i.pointer
            .hover_pos()
            .or_else(|| i.pointer.interact_pos())
            .unwrap_or_default()
    });
    let hwnd = unsafe { windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow() };
    let ppp = ctx.pixels_per_point();
    let _ = sc_shell::context::show_shell_context_menu(
        hwnd,
        paths,
        (pos.x * ppp) as i32,
        (pos.y * ppp) as i32,
        extended,
    );
}

fn pane_background_menu(app: &mut ScApp, ctx: &egui::Context) {
    let Some((pane, pos)) = app.pane_bg_menu else {
        return;
    };
    let mut close = false;
    let mut new_folder = false;
    let mut new_file = false;
    let mut paste = false;
    let mut refresh = false;
    let mut properties = false;
    let mut toggle_fav = false;
    let fav_path = app.panes[pane].tab().path.clone();
    let in_recycle = sc_shell::recycle::is_recycle_path(&fav_path);
    let is_fav = app.is_favorite(&fav_path);
    let area = egui::Area::new(egui::Id::new("pane-bg-menu"))
        .order(egui::Order::Foreground)
        .fixed_pos(pos)
        .constrain(true)
        .show(ctx, |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.set_min_width(180.0);
                if !in_recycle {
                if ui
                    .button(format!("New folder\t{}", app.settings.keymap.new_folder.label()))
                    .clicked()
                {
                    new_folder = true;
                    close = true;
                }
                if ui.button("New file").clicked() {
                    new_file = true;
                    close = true;
                }
                let can_paste = sc_shell::clipboard::clipboard_has_files();
                if ui
                    .add_enabled(
                        can_paste,
                        egui::Button::new(format!("Paste\t{}", app.settings.keymap.paste.label())),
                    )
                    .clicked()
                {
                    paste = true;
                    close = true;
                }
                ui.separator();
                }
                let fav_label = if is_fav {
                    "Remove from favorites"
                } else {
                    "Add to favorites"
                };
                if ui.button(fav_label).clicked() {
                    toggle_fav = true;
                    close = true;
                }
                ui.separator();
                if ui
                    .button(format!("Refresh\t{}", app.settings.keymap.refresh.label()))
                    .clicked()
                {
                    refresh = true;
                    close = true;
                }
                if ui.button("Properties").clicked() {
                    properties = true;
                    close = true;
                }
            });
        });
    if close {
        app.pane_bg_menu = None;
    } else if ctx.input(|i| i.pointer.primary_clicked() || i.key_pressed(Key::Escape))
        && !area.response.contains_pointer()
    {
        app.pane_bg_menu = None;
    }
    if new_folder {
        app.begin_new_folder(pane);
    }
    if new_file {
        app.begin_new_file(pane);
    }
    if toggle_fav {
        app.toggle_favorite(fav_path);
    }
    if paste {
        app.paste_into(pane);
    }
    if refresh {
        app.request_listing(pane, true);
    }
    if properties {
        let path = app.panes[pane].tab().path.clone();
        let _ = sc_shell::context::shell_properties(&path);
    }
}

fn format_time(filetime: u64) -> String {
    if filetime == 0 {
        return String::new();
    }
    thread_local! {
        static CACHE: std::cell::RefCell<std::collections::HashMap<u64, String>> =
            std::cell::RefCell::new(std::collections::HashMap::new());
    }
    CACHE.with(|c| {
        let mut cache = c.borrow_mut();
        if let Some(s) = cache.get(&filetime) {
            return s.clone();
        }
        let local = sc_shell::enumerate::filetime_utc_to_local(filetime);
        let unix = sc_core::entry::filetime_to_unix_secs(local);
        let s = format_unix_time(unix);
        if cache.len() > 8192 {
            cache.clear();
        }
        cache.insert(filetime, s.clone());
        s
    })
}

/// Minimal date formatting without a chrono dependency.
fn format_unix_time(unix: i64) -> String {
    if unix <= 0 {
        return String::new();
    }
    let days = unix / 86400;
    let secs_of_day = unix % 86400;
    // civil_from_days (Howard Hinnant's algorithm).
    let z = days + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}",
        y,
        m,
        d,
        secs_of_day / 3600,
        (secs_of_day % 3600) / 60
    )
}

// ---------------------------------------------------------------- panels

fn breadcrumb_parts(path: &std::path::Path) -> Vec<(String, PathBuf)> {
    if sc_shell::recycle::is_recycle_path(path) {
        return vec![("Recycle Bin".into(), sc_shell::recycle::recycle_root())];
    }
    let mut parts = Vec::new();
    let mut acc = PathBuf::new();
    for (i, c) in path.components().enumerate() {
        acc.push(c.as_os_str());
        let label = if i == 0 {
            acc.display().to_string()
        } else {
            c.as_os_str().to_string_lossy().into_owned()
        };
        if label.is_empty() {
            continue;
        }
        parts.push((label, acc.clone()));
    }
    if parts.is_empty() {
        parts.push((path.display().to_string(), path.to_path_buf()));
    }
    parts
}

fn ops_panel(app: &mut ScApp, ui: &mut Ui) {
    let has_active = app.ops_view.iter().any(|o| !o.finished);
    let has_errors = app.ops_view.iter().any(|o| o.error.is_some());
    if !has_active && !has_errors {
        return;
    }
    egui::Panel::bottom("ops").show(ui, |ui| {
        let mut clear_finished = false;
        let mut pause_id: Option<u64> = None;
        let mut resume_id: Option<u64> = None;
        let mut cancel_id: Option<u64> = None;
        let mut pause_all = false;
        let mut resume_all = false;
        let mut cancel_all = false;
        let selected = app.ops_selected;
        let rows: Vec<(u64, String, bool, Option<String>, f32, u64, u64, String, bool, f64)> = app
            .ops_view
            .iter()
            .filter(|o| !o.finished || o.error.is_some())
            .map(|op| {
                let frac = if op.total_bytes > 0 {
                    op.done_bytes as f32 / op.total_bytes as f32
                } else if op.total_files > 0 {
                    op.done_files as f32 / op.total_files as f32
                } else {
                    0.0
                };
                (
                    op.op_id,
                    op.label.clone(),
                    op.finished,
                    op.error.clone(),
                    frac,
                    op.done_bytes,
                    op.total_bytes,
                    op.current.clone(),
                    app.ops.is_paused(op.op_id),
                    op.started.elapsed().as_secs_f64(),
                )
            })
            .collect();
        for (op_id, label, finished, error, frac, done_bytes, total_bytes, current, paused, elapsed) in rows {
            ui.horizontal(|ui| {
                let sel = selected == Some(op_id);
                if ui.selectable_label(sel, "●").clicked() {
                    app.ops_selected = Some(op_id);
                }
                if let Some(err) = error {
                    ui.colored_label(app.theme.error, format!("{label} — {err}"));
                } else {
                    ui.label(&label);
                    ui.add(
                        egui::ProgressBar::new(frac)
                            .desired_width(220.0)
                            .text(format!(
                                "{} / {}",
                                format_size(done_bytes),
                                format_size(total_bytes)
                            )),
                    );
                    ui.weak(&current);
                    if elapsed > 0.5 && done_bytes > 0 {
                        let speed = done_bytes as f64 / elapsed;
                        ui.weak(format!("{}/s", format_size(speed as u64)));
                    }
                    if !finished {
                        if paused {
                            if ui.small_button("Resume").clicked() {
                                resume_id = Some(op_id);
                            }
                        } else if ui.small_button("Pause").clicked() {
                            pause_id = Some(op_id);
                        }
                        if ui.small_button("Cancel").clicked() {
                            cancel_id = Some(op_id);
                        }
                    }
                }
            });
        }
        ui.horizontal(|ui| {
            if has_active {
                if app.ops.any_paused() {
                    if ui.button("Resume all").clicked() {
                        resume_all = true;
                    }
                } else if ui.button("Pause all").clicked() {
                    pause_all = true;
                }
                if ui.button("Cancel all").clicked() {
                    cancel_all = true;
                }
            }
            if has_errors && ui.button("Dismiss errors").clicked() {
                clear_finished = true;
            }
        });
        if let Some(id) = pause_id {
            app.ops.pause(id);
        }
        if let Some(id) = resume_id {
            app.ops.resume(id);
        }
        if let Some(id) = cancel_id {
            app.ops.cancel(id);
        }
        if pause_all {
            app.ops.pause_all();
        }
        if resume_all {
            app.ops.resume_all();
        }
        if cancel_all {
            app.ops.cancel_all();
        }
        if clear_finished {
            app.ops_view.retain(|o| !o.finished);
        }
    });
}

fn status_bar(app: &mut ScApp, ui: &mut Ui) {
    app.maybe_refresh_places();
    egui::Panel::bottom("status").show(ui, |ui| {
        // Rule above the status bar, for the same reason as the one under the
        // tab strip: on AMOLED there is no fill change to separate them.
        let top = ui.max_rect();
        ui.painter().hline(
            top.x_range(),
            top.min.y - 1.0,
            egui::Stroke::new(1.0, app.theme.separator),
        );
        ui.horizontal(|ui| {
            let path = app.active_tab().path.clone();
            let (dirs, files, file_bytes, view_len, has_filter, loading, sel, first_name, entries, selection) = {
                let tab = app.active_tab();
                let first_name = tab.selection.iter().next().and_then(|&i| {
                    tab.snapshot.entries.get(i as usize).map(|e| e.name.clone())
                });
                (
                    tab.snapshot.dir_count,
                    tab.snapshot.file_count,
                    tab.snapshot.file_bytes,
                    tab.view.len(),
                    !tab.filter.is_empty(),
                    tab.loading,
                    tab.selection.len(),
                    first_name,
                    tab.snapshot.entries.clone(),
                    tab.selection.clone(),
                )
            };
            let sel_bytes: u64 = selection
                .iter()
                .filter_map(|&i| {
                    let e = entries.get(i as usize)?;
                    if e.is_dir() {
                        app.folder_sizes.get(&path.join(&e.name)).copied()
                    } else {
                        Some(e.size)
                    }
                })
                .sum();
            // Hierarchy: values carry the weight, labels recede, and the
            // selection count — the number you watch during a copy — is accent.
            let theme = app.theme;
            let dim = theme.text_weak;
            ui.spacing_mut().item_spacing.x = 4.0;
            let sep = |ui: &mut Ui| {
                ui.add(
                    egui::Label::new(RichText::new("·").color(dim.gamma_multiply(0.6)))
                        .selectable(false),
                );
            };
            let val = |ui: &mut Ui, s: String| {
                ui.add(egui::Label::new(RichText::new(s).color(theme.text)).selectable(false));
            };
            let lbl = |ui: &mut Ui, s: &str| {
                ui.add(egui::Label::new(RichText::new(s).color(dim)).selectable(false));
            };
            val(ui, dirs.to_string());
            lbl(ui, "folders");
            val(ui, files.to_string());
            lbl(ui, "files");
            sep(ui);
            val(ui, format_size(file_bytes));
            if has_filter {
                sep(ui);
                val(ui, view_len.to_string());
                lbl(ui, "shown");
            }
            sep(ui);
            let sel_color = if sel > 0 { theme.accent } else { dim };
            ui.add(
                egui::Label::new(RichText::new(sel.to_string()).color(sel_color).strong())
                    .selectable(false),
            );
            ui.add(
                egui::Label::new(RichText::new("selected").color(if sel > 0 {
                    theme.accent.gamma_multiply(0.85)
                } else {
                    dim
                }))
                .selectable(false),
            );
            if sel > 0 {
                ui.add(
                    egui::Label::new(
                        RichText::new(format!("({})", format_size(sel_bytes))).color(dim),
                    )
                    .selectable(false),
                );
            }
            if loading {
                sep(ui);
                ui.add(
                    egui::Label::new(RichText::new("loading…").color(theme.accent))
                        .selectable(false),
                );
            }
            if sc_shell::recycle::is_recycle_path(&path) {
                if let Some(name) = first_name {
                    if let Some(item) = app.recycle_meta.get(&name) {
                        if let Some(orig) = &item.original_path {
                            sep(ui);
                            lbl(ui, "from");
                            val(ui, orig.display().to_string());
                        }
                    }
                }
            }

            let vol = sc_shell::volumes::volume_for_path(&app.volumes, &path)
                .map(|v| {
                    let letter = v.root.to_string_lossy().chars().next().unwrap_or('?');
                    (
                        format!("{letter}: {} free", format_size(v.free_bytes)),
                        format!(
                            "{} free of {}",
                            format_size(v.free_bytes),
                            format_size(v.total_bytes)
                        ),
                    )
                });

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let started = app.start_time;
                let ms = *app
                    .startup_ms
                    .get_or_insert_with(|| started.elapsed().as_millis() as u64);
                ui.weak(format!("{ms} ms"));
                ui.separator();
                match app.engine.index.status() {
                    _ if sc_shell::everything::is_running() => {
                        ui.weak("search: Everything");
                    }
                    sc_index::search::IndexStatus::Building => {
                        ui.weak("index: building…");
                    }
                    sc_index::search::IndexStatus::Ready { entries, mft } => {
                        ui.weak(format!(
                            "index: {} entries ({})",
                            entries,
                            if mft { "MFT" } else { "walk" }
                        ));
                    }
                    sc_index::search::IndexStatus::Unavailable => {
                        ui.weak("index: unavailable");
                    }
                }
                if let Some((text, tip)) = vol {
                    ui.separator();
                    ui.weak(text).on_hover_text(tip);
                }
            });
        });
    });
}

// ---------------------------------------------------------------- overlays

fn search_overlay(app: &mut ScApp, ctx: &egui::Context) {
    if !app.search.open {
        return;
    }
    let mut open = app.search.open;
    let mut dismiss = false;
    let skip_outside = app.search.focus_requested;

    egui::Area::new(egui::Id::new("search-backdrop"))
        .order(egui::Order::Foreground)
        .fixed_pos(ctx.content_rect().min)
        .interactable(true)
        .show(ctx, |ui| {
            let rect = ctx.content_rect();
            let resp = ui.allocate_rect(rect, Sense::click());
            ui.painter()
                .rect_filled(rect, 0.0, Color32::from_black_alpha(64));
            if resp.clicked() && !skip_outside {
                dismiss = true;
            }
        });

    egui::Window::new("Search")
        .open(&mut open)
        .order(egui::Order::Tooltip)
        .anchor(Align2::CENTER_TOP, [0.0, 48.0])
        .resizable(true)
        .collapsible(false)
        .default_width(620.0)
        .show(ctx, |ui| {
            let n = app.search.results.len();
            let mut arrow_moved = false;
            let mut activate: Option<(PathBuf, bool)> = None;
            let mut close_after = false;
            if ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Tab)) {
                app.search.mode = app.search.mode.next();
                app.run_search();
            } else if ui.input_mut(|i| i.consume_key(Modifiers::SHIFT, Key::Tab)) {
                app.search.mode = app.search.mode.prev();
                app.run_search();
            }
            if n > 0 {
                if ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::ArrowDown)) {
                    app.search.selected = (app.search.selected + 1).min(n - 1);
                    arrow_moved = true;
                }
                if ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::ArrowUp)) {
                    app.search.selected = app.search.selected.saturating_sub(1);
                    arrow_moved = true;
                }
                if ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Enter)) {
                    if let Some((path, is_dir)) = app.search.results.get(app.search.selected) {
                        activate = Some((path.clone(), *is_dir));
                        close_after = true;
                    }
                }
            }
            ui.horizontal(|ui| {
                let mut output = TextEdit::singleline(&mut app.search.query)
                    .id(egui::Id::new("search-query"))
                    .hint_text(r"c:/ folder/ *.txt")
                    .desired_width(320.0)
                    .show(ui);
                select_all_on_focus(ui, &mut output, &mut app.search.focus_requested);
                let changed = output.response.changed();
                let mode_before = app.search.mode;
                ui.selectable_value(&mut app.search.mode, SearchMode::NameHere, "This folder");
                ui.selectable_value(&mut app.search.mode, SearchMode::NameGlobal, "Everywhere");
                ui.selectable_value(&mut app.search.mode, SearchMode::Content, "Content");
                ui.weak("Tab");
                if changed || app.search.mode != mode_before {
                    app.run_search();
                }
                if app.search.running {
                    ui.spinner();
                }
            });
            if app.search.mode != SearchMode::Content {
                ui.weak("Spaces AND the path  ·  * ? wildcards  ·  | OR  ·  ! NOT  ·  \"quoted phrase\"");
            }
            ui.separator();
            let results = app.search.results.clone();
            egui::ScrollArea::vertical().max_height(360.0).show_rows(
                ui,
                20.0,
                results.len(),
                |ui, range| {
                    for i in range {
                        let (path, is_dir) = &results[i];
                        let icon = if *is_dir { "📁" } else { "📄" };
                        let resp = ui.selectable_label(
                            i == app.search.selected,
                            format!("{icon} {}", path.display()),
                        );
                        if arrow_moved && i == app.search.selected {
                            resp.scroll_to_me(Some(egui::Align::Center));
                        }
                        if resp.clicked() {
                            app.search.selected = i;
                        }
                        if resp.double_clicked() {
                            activate = Some((path.clone(), *is_dir));
                            close_after = true;
                        }
                        if resp.middle_clicked() {
                            app.search.selected = i;
                            activate = Some((path.clone(), *is_dir));
                        }
                    }
                },
            );
            if !results.is_empty() {
                ui.weak(format!("{} result(s)", results.len()));
            }
            if let Some((path, is_dir)) = activate {
                app.activate_search_hit(path, is_dir);
                if close_after {
                    dismiss = true;
                }
            }
        });
    if dismiss || ctx.input(|i| i.key_pressed(Key::Escape)) {
        open = false;
    }
    app.search.open = open;
}

fn palette_overlay(app: &mut ScApp, ctx: &egui::Context) {
    if !app.palette.open {
        return;
    }
    let mut dismiss = false;
    let skip_outside = app.palette.focus_requested;

    egui::Area::new(egui::Id::new("palette-backdrop"))
        .order(egui::Order::Foreground)
        .fixed_pos(ctx.content_rect().min)
        .interactable(true)
        .show(ctx, |ui| {
            let rect = ctx.content_rect();
            let resp = ui.allocate_rect(rect, Sense::click());
            ui.painter()
                .rect_filled(rect, 0.0, Color32::from_black_alpha(64));
            if resp.clicked() && !skip_outside {
                dismiss = true;
            }
        });

    egui::Window::new("Quick jump")
        .title_bar(false)
        .order(egui::Order::Tooltip)
        .anchor(Align2::CENTER_TOP, [0.0, 80.0])
        .fixed_size([520.0, 320.0])
        .collapsible(false)
        .show(ctx, |ui| {
            let n = app.palette.results.len();
            let mut arrow_moved = false;
            if n > 0 {
                app.palette.selected = app.palette.selected.min(n - 1);
                if ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::ArrowDown)) {
                    app.palette.selected = (app.palette.selected + 1).min(n - 1);
                    arrow_moved = true;
                }
                if ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::ArrowUp)) {
                    app.palette.selected = app.palette.selected.saturating_sub(1);
                    arrow_moved = true;
                }
            }
            let mut output = TextEdit::singleline(&mut app.palette.query)
                .id(egui::Id::new("palette-query"))
                .hint_text("Jump to folder…")
                .desired_width(f32::INFINITY)
                .show(ui);
            select_all_on_focus(ui, &mut output, &mut app.palette.focus_requested);
            if output.response.changed() {
                app.run_palette();
                arrow_moved = true;
            }
            ui.separator();
            let n = app.palette.results.len();
            if n > 0 {
                app.palette.selected = app.palette.selected.min(n - 1);
            }
            let mut go: Option<PathBuf> = None;
            let row_h = ui.spacing().interact_size.y;
            let row_step = row_h + ui.spacing().item_spacing.y;
            let list_h = ui.available_height().max(80.0);
            let mut list = egui::ScrollArea::vertical()
                .id_salt("palette-results")
                .max_height(list_h)
                .auto_shrink([false, false])
                .animated(false);
            if arrow_moved && n > 0 {
                let content_h = (n as f32 * row_step - ui.spacing().item_spacing.y).max(0.0);
                let max_off = (content_h - list_h).max(0.0);
                let target = (app.palette.selected as f32 * row_step - (list_h - row_h) * 0.5)
                    .clamp(0.0, max_off);
                list = list.vertical_scroll_offset(target);
            }
            list.show_rows(ui, row_h, n, |ui, range| {
                for i in range {
                    let (path, _) = &app.palette.results[i];
                    let resp = ui.add(
                        egui::Button::selectable(
                            i == app.palette.selected,
                            path.display().to_string(),
                        )
                        .truncate(),
                    );
                    if arrow_moved && i == app.palette.selected {
                        resp.scroll_to_me(Some(egui::Align::Center));
                    }
                    if resp.clicked() {
                        app.palette.selected = i;
                        go = Some(path.clone());
                    }
                }
            });
            if ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Enter)) {
                if let Some((p, _)) = app.palette.results.get(app.palette.selected) {
                    go = Some(p.clone());
                }
            }
            if let Some(p) = go {
                let pane = app.active_pane;
                app.navigate(pane, p);
                dismiss = true;
            }
            if ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape)) {
                dismiss = true;
            }
        });

    if dismiss || ctx.input(|i| i.key_pressed(Key::Escape)) {
        app.palette.open = false;
        if ctx.input(|i| i.key_pressed(Key::Escape)) {
            ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape));
        }
    }
}

fn toasts(app: &mut ScApp, ctx: &egui::Context) {
    if app.toasts.is_empty() {
        return;
    }
    egui::Area::new("toasts".into())
        .anchor(Align2::RIGHT_BOTTOM, [-12.0, -40.0])
        .show(ctx, |ui| {
            for (msg, _, is_error) in app.toasts.iter().rev().take(4) {
                let color = if *is_error { app.theme.error } else { app.theme.ok };
                egui::Frame::new()
                    .fill(app.theme.hover_bg)
                    .stroke(egui::Stroke::new(1.0, color))
                    .corner_radius(4)
                    .inner_margin(8.0)
                    .show(ui, |ui| {
                        ui.colored_label(color, msg);
                    });
                ui.add_space(4.0);
            }
        });
}

fn pointer_outside_window(hwnd: Option<isize>) -> bool {
    sc_shell::drag::cursor_outside_window(hwnd)
}

/// Hand egui the mouse-up that a modal OLE drag swallowed.
///
/// `DoDragDrop` runs its own message loop and consumes the release, so egui
/// comes back believing the button is still down. Rather than *guessing* at
/// that state every frame — the previous approach, which also fired on ordinary
/// releases and wiped drag payloads before they could be used — this runs only
/// when [`handle_file_drops`] actually returned from a drag, which is the one
/// situation where the release is known to be missing.
pub(crate) fn finish_ole_drag_gesture(
    app: &mut ScApp,
    ctx: &egui::Context,
    raw: &mut egui::RawInput,
) {
    if !std::mem::take(&mut app.pointer_reset) {
        return;
    }
    app.end_list_gesture();
    egui::DragAndDrop::clear_payload(ctx);
    // Drop a stale press so egui cannot pair it with the synthetic release.
    raw.events.retain(|e| {
        !matches!(
            e,
            egui::Event::PointerButton {
                button: egui::PointerButton::Primary,
                pressed: true,
                ..
            }
        )
    });
    let pos = ctx.pointer_latest_pos().unwrap_or(egui::pos2(0.0, 0.0));
    let modifiers = ctx.input(|i| i.modifiers);
    raw.events.push(egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed: false,
        modifiers,
    });
}

fn handle_file_drops(app: &mut ScApp, ctx: &egui::Context) {
    // In-app drops are handled by the widgets themselves (see `file_drop_target`),
    // which is why there is no global "what was under the pointer" lookup here.
    // This function only bridges to and from the OS.
    if egui::DragAndDrop::has_payload_of_type::<FileDrag>(ctx) && !app.ole_drag_active {
        // Poll while a drag is live so the badge tracks the cursor and the
        // window-exit test below stays responsive.
        ctx.request_repaint();
        // Both of these are pure queries — unlike SetCapture, they do not
        // perturb the input state egui is reading.
        let button_down = async_key_down(0x01); // VK_LBUTTON
        let left_window = pointer_outside_window(app.preview.parent_hwnd)
            || ctx.input(|i| {
                i.pointer
                    .latest_pos()
                    .is_none_or(|p| !i.viewport_rect().contains(p))
            });
        if button_down && left_window {
            if let Some(drag) = egui::DragAndDrop::take_payload::<FileDrag>(ctx) {
                app.ole_drag_active = true;
                // Blocking: Windows pumps its own message loop until the drop.
                let moved = sc_shell::drag::start_drag(&drag.paths) == Some(true);
                app.ole_drag_active = false;
                // The release happened inside that modal loop; tell the input
                // hook to synthesise it on the next frame.
                app.pointer_reset = true;
                if moved {
                    app.request_listing(drag.from_pane.min(app.panes.len() - 1), false);
                }
            }
        }
    }

    // Files dragged in from Explorer land as a viewport-level event, with no
    // widget to attach them to, so this one does need a pane lookup.
    let dropped: Vec<PathBuf> = ctx.input(|i| {
        i.raw
            .dropped_files
            .iter()
            .map(|f| f.path().to_path_buf())
            .collect()
    });
    if dropped.is_empty() {
        return;
    }
    let pos = ctx.input(|i| i.pointer.hover_pos()).unwrap_or_default();
    let pane = app
        .pane_rects
        .iter()
        .position(|r| r.contains(pos))
        .unwrap_or(app.active_pane);
    let dest = app.panes[pane.min(app.panes.len() - 1)].tab().path.clone();
    app.drop_files_into(dropped, dest, false);
}

#[cfg(test)]
mod tests {
    use super::TOOLBAR_ROW_H;

    /// Runs a frame against the real root `Ui` — the same one eframe hands
    /// `App::ui`, and therefore the same situation the toolbar row sits in —
    /// and reports the height each layout style claims.
    fn row_heights() -> (f32, f32) {
        let ctx = egui::Context::default();
        let mut raw = egui::RawInput::default();
        raw.screen_rect = Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(1000.0, 800.0),
        ));

        let mut unbounded = 0.0;
        let mut bounded = 0.0;
        let mut out = ctx.run_ui(raw, |ui| {
            // What the toolbar row used to do.
            unbounded = ui
                .with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label("x");
                })
                .response
                .rect
                .height();
            // What it does now.
            bounded = ui
                .allocate_ui_with_layout(
                    egui::vec2(ui.available_width(), TOOLBAR_ROW_H),
                    egui::Layout::right_to_left(egui::Align::Center),
                    |ui| {
                        ui.label("x");
                    },
                )
                .response
                .rect
                .height();
        });
        // The frame rasterised glyphs into the font atlas; egui asserts that
        // the resulting texture delta is consumed rather than dropped.
        out.textures_delta.clear();
        (unbounded, bounded)
    }

    #[test]
    fn toolbar_row_does_not_claim_the_whole_window() {
        let (_unbounded, bounded) = row_heights();
        assert!(
            bounded <= TOOLBAR_ROW_H + 4.0,
            "toolbar row claimed {bounded}pt, expected about {TOOLBAR_ROW_H}pt"
        );
    }

    /// Documents *why* the explicit allocation is load-bearing: `with_layout`
    /// hands the child the entire available rect, so inside a top panel the row
    /// grows to the full window height and drags the panel down with it.
    #[test]
    fn with_layout_would_over_claim_the_height() {
        let (unbounded, bounded) = row_heights();
        assert!(
            unbounded > bounded * 4.0,
            "expected with_layout to over-claim (got {unbounded}pt vs {bounded}pt); \
             if egui changed this, the explicit allocation is no longer needed"
        );
    }
}
