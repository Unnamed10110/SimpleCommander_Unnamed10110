//! Modal dialogs: conflict resolution, delete confirmation, batch rename,
//! tags/comment editor, plugin manager, plugin output, about, new file/folder.

use crate::app::ScApp;
use crate::ui::select_all_on_focus;
use egui::{Align2, Key, RichText, TextEdit};
use sc_ops::queue::{ConflictResolution, Operation};
use std::path::PathBuf;

pub fn draw(app: &mut ScApp, ctx: &egui::Context) {
    new_item_dialog(app, ctx);
    conflict_dialog(app, ctx);
    delete_confirm(app, ctx);
    batch_rename(app, ctx);
    tag_editor(app, ctx);
    plugin_manager(app, ctx);
    plugin_output(app, ctx);
    columns_dialog(app, ctx);
    about(app, ctx);
    everything_prompt(app, ctx);
    folder_compare(app, ctx);
}

fn new_item_dialog(app: &mut ScApp, ctx: &egui::Context) {
    if app.new_item.is_none() {
        return;
    }
    let err_color = app.theme.error;
    let is_folder = app.new_item.as_ref().map(|p| p.is_folder).unwrap_or(false);
    let title = if is_folder { "New folder" } else { "New file" };
    let mut create = false;
    let mut cancel = false;
    egui::Window::new(title)
        .collapsible(false)
        .resizable(false)
        .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
        .order(egui::Order::Foreground)
        .show(ctx, |ui| {
            let Some(prompt) = app.new_item.as_mut() else {
                return;
            };
            ui.label("Name");
            let mut output = TextEdit::singleline(&mut prompt.name)
                .id(egui::Id::new("new-item-name"))
                .desired_width(280.0)
                .show(ui);
            select_all_on_focus(ui, &mut output, &mut prompt.focus_requested);
            if let Some(err) = &prompt.error {
                ui.colored_label(err_color, err);
            }
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button("Create").clicked() {
                    create = true;
                }
                if ui.button("Cancel").clicked() {
                    cancel = true;
                }
            });
            if ui.input(|i| i.key_pressed(Key::Enter)) {
                create = true;
            }
            if ui.input(|i| i.key_pressed(Key::Escape)) {
                cancel = true;
            }
        });
    if cancel {
        app.new_item = None;
    } else if create {
        app.submit_new_item();
    }
}

fn conflict_dialog(app: &mut ScApp, ctx: &egui::Context) {
    let Some(c) = &mut app.conflict else { return };
    let mut answer: Option<ConflictResolution> = None;
    let apply_all = &mut c.apply_to_all;
    egui::Window::new("File exists")
        .collapsible(false)
        .resizable(false)
        .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            ui.label("Target already exists:");
            ui.label(RichText::new(c.dest.display().to_string()).strong());
            ui.weak(format!("Source: {}", c.source.display()));
            ui.add_space(6.0);
            ui.checkbox(apply_all, "Apply to all conflicts in this operation");
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if ui.button("Overwrite").clicked() {
                    answer = Some(ConflictResolution::Overwrite);
                }
                if ui.button("Keep both (rename)").clicked() {
                    answer = Some(ConflictResolution::AutoRename);
                }
                if ui.button("Skip").clicked() {
                    answer = Some(ConflictResolution::Skip);
                }
                if ui.button("Cancel operation").clicked() {
                    answer = Some(ConflictResolution::Cancel);
                }
            });
        });
    if let Some(res) = answer {
        let apply = app.conflict.as_ref().map(|c| c.apply_to_all).unwrap_or(false);
        app.answer_conflict(res, apply);
    }
}

fn delete_confirm(app: &mut ScApp, ctx: &egui::Context) {
    let Some((paths, permanent)) = &app.pending_delete else { return };
    let n = paths.len();
    let permanent = *permanent;
    let mut decided: Option<bool> = None;
    egui::Window::new("Delete permanently?")
        .collapsible(false)
        .resizable(false)
        .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            ui.label(format!(
                "Permanently delete {n} item(s)? This cannot be undone."
            ));
            if n == 1 {
                ui.label(RichText::new(paths[0].display().to_string()).weak());
            }
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui
                    .button(RichText::new("Delete").color(app.theme.error))
                    .clicked()
                {
                    decided = Some(true);
                }
                if ui.button("Cancel").clicked() {
                    decided = Some(false);
                }
            });
        });
    if let Some(go) = decided {
        if let Some((paths, _)) = app.pending_delete.take() {
            if go {
                app.submit_op(Operation::Delete { paths, recycle: !permanent });
            }
        }
    }
}

// ---------------------------------------------------------------- batch rename

fn apply_case(s: &str, mode: usize) -> String {
    match mode {
        1 => s.to_lowercase(),
        2 => s.to_uppercase(),
        3 => {
            // Title Case per word.
            let mut out = String::with_capacity(s.len());
            let mut new_word = true;
            for ch in s.chars() {
                if ch.is_alphanumeric() {
                    if new_word {
                        out.extend(ch.to_uppercase());
                    } else {
                        out.extend(ch.to_lowercase());
                    }
                    new_word = false;
                } else {
                    out.push(ch);
                    new_word = true;
                }
            }
            out
        }
        _ => s.to_string(),
    }
}

/// Compute the new name for one file under the batch-rename settings.
pub fn batch_new_name(
    original: &str,
    pattern: &str,
    find: &str,
    replace: &str,
    use_regex: bool,
    case: usize,
    counter: u32,
) -> Result<String, String> {
    let (stem, ext) = match original.rfind('.') {
        Some(i) if i > 0 => (&original[..i], &original[i + 1..]),
        _ => (original, ""),
    };
    let mut name = pattern
        .replace("<name>", stem)
        .replace("<ext>", ext)
        .replace("<#>", &counter.to_string())
        .replace("<##>", &format!("{counter:02}"))
        .replace("<###>", &format!("{counter:03}"));
    if !find.is_empty() {
        if use_regex {
            let re = regex::Regex::new(find).map_err(|e| e.to_string())?;
            name = re.replace_all(&name, replace).into_owned();
        } else {
            name = name.replace(find, replace);
        }
    }
    name = apply_case(&name, case);
    if !ext.is_empty() && !pattern.contains("<ext>") {
        name.push('.');
        name.push_str(ext);
    }
    Ok(name)
}

fn batch_rename(app: &mut ScApp, ctx: &egui::Context) {
    if !app.batch_rename.open {
        return;
    }
    let mut open = true;
    let mut apply = false;
    egui::Window::new("Batch rename")
        .open(&mut open)
        .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
        .default_width(560.0)
        .show(ctx, |ui| {
            let br = &mut app.batch_rename;
            egui::Grid::new("br-grid").num_columns(2).show(ui, |ui| {
                ui.label("Pattern");
                ui.add(
                    TextEdit::singleline(&mut br.pattern)
                        .hint_text("<name> tokens: <name> <ext> <#> <##> <###>")
                        .desired_width(f32::INFINITY),
                );
                ui.end_row();
                ui.label("Find");
                ui.add(TextEdit::singleline(&mut br.find).desired_width(f32::INFINITY));
                ui.end_row();
                ui.label("Replace");
                ui.add(TextEdit::singleline(&mut br.replace).desired_width(f32::INFINITY));
                ui.end_row();
                ui.label("Options");
                ui.horizontal(|ui| {
                    ui.checkbox(&mut br.use_regex, "Regex");
                    egui::ComboBox::from_id_salt("br-case")
                        .selected_text(["Keep case", "lowercase", "UPPERCASE", "Title Case"][br.case])
                        .show_ui(ui, |ui| {
                            for (i, label) in
                                ["Keep case", "lowercase", "UPPERCASE", "Title Case"]
                                    .iter()
                                    .enumerate()
                            {
                                ui.selectable_value(&mut br.case, i, *label);
                            }
                        });
                    ui.label("Counter start:");
                    ui.add(egui::DragValue::new(&mut br.counter_start));
                });
                ui.end_row();
            });
            ui.separator();
            // Live preview.
            let mut error = None;
            egui::ScrollArea::vertical().max_height(260.0).show(ui, |ui| {
                egui::Grid::new("br-preview").num_columns(2).striped(true).show(ui, |ui| {
                    for (i, item) in br.items.iter().enumerate() {
                        let orig = item
                            .file_name()
                            .map(|n| n.to_string_lossy().into_owned())
                            .unwrap_or_default();
                        match batch_new_name(
                            &orig,
                            &br.pattern,
                            &br.find,
                            &br.replace,
                            br.use_regex,
                            br.case,
                            br.counter_start + i as u32,
                        ) {
                            Ok(new_name) => {
                                ui.label(&orig);
                                if new_name == orig {
                                    ui.weak(new_name);
                                } else {
                                    ui.label(RichText::new(new_name).color(app.theme.accent));
                                }
                            }
                            Err(e) => {
                                ui.label(&orig);
                                ui.colored_label(app.theme.error, e.clone());
                                error = Some(e);
                            }
                        }
                        ui.end_row();
                    }
                });
            });
            br.error = error;
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(br.error.is_none(), egui::Button::new("Rename all"))
                    .clicked()
                {
                    apply = true;
                }
            });
        });
    if apply {
        let br = &app.batch_rename;
        let mut ops: Vec<Operation> = Vec::new();
        for (i, item) in br.items.iter().enumerate() {
            let orig = item
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            if let Ok(new_name) = batch_new_name(
                &orig,
                &br.pattern,
                &br.find,
                &br.replace,
                br.use_regex,
                br.case,
                br.counter_start + i as u32,
            ) {
                if new_name != orig && !new_name.is_empty() {
                    if let Some(parent) = item.parent() {
                        ops.push(Operation::Rename {
                            from: item.clone(),
                            to: parent.join(new_name),
                        });
                    }
                }
            }
        }
        for op in ops {
            app.submit_op(op);
        }
        app.batch_rename.open = false;
    } else {
        app.batch_rename.open = open && app.batch_rename.open;
    }
}

fn tag_editor(app: &mut ScApp, ctx: &egui::Context) {
    let Some(te) = &mut app.tag_edit else { return };
    if !te.open {
        app.tag_edit = None;
        return;
    }
    let mut save = false;
    let mut open = te.open;
    egui::Window::new("Tags & comment")
        .open(&mut open)
        .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
        .default_width(420.0)
        .show(ctx, |ui| {
            ui.label(RichText::new(te.path.display().to_string()).weak());
            ui.add_space(4.0);
            ui.label("Tags (comma separated):");
            ui.add(TextEdit::singleline(&mut te.tags).desired_width(f32::INFINITY));
            ui.label("Comment:");
            ui.add(
                TextEdit::multiline(&mut te.comment)
                    .desired_rows(3)
                    .desired_width(f32::INFINITY),
            );
            ui.add_space(6.0);
            if ui.button("Save").clicked() {
                save = true;
            }
        });
    if save {
        if let Some(te) = app.tag_edit.take() {
            app.tags.set_tags(&te.path, te.tags.clone());
            app.tags.set_comment(&te.path, te.comment.clone());
        }
    } else if let Some(te) = &mut app.tag_edit {
        te.open = open;
    }
}

fn plugin_manager(app: &mut ScApp, ctx: &egui::Context) {
    if !app.show_plugin_manager {
        return;
    }
    let mut open = app.show_plugin_manager;
    let mut install_path: Option<PathBuf> = None;
    let mut toggle: Option<(PathBuf, bool)> = None;
    let mut grant: Option<(PathBuf, String)> = None;
    egui::Window::new("Plugin manager")
        .open(&mut open)
        .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
        .default_width(560.0)
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                if ui.button("Install plugin (.wasm)…").clicked() {
                    if let Some(file) = rfd::FileDialog::new()
                        .add_filter("WASM plugin", &["wasm"])
                        .set_directory(crate::config::plugins_dir())
                        .pick_file()
                    {
                        install_path = Some(file);
                    }
                }
                ui.weak("Plugins are sandboxed; grant permissions explicitly.");
            });
            ui.separator();
            let host = app.engine.plugins.read();
            if host.plugins.is_empty() {
                ui.weak("No plugins installed.");
            }
            for p in &host.plugins {
                ui.horizontal(|ui| {
                    let mut enabled = p.record.enabled;
                    if ui.checkbox(&mut enabled, "").changed() {
                        toggle = Some((p.record.path.clone(), enabled));
                    }
                    ui.label(RichText::new(&p.manifest.name).strong());
                    ui.weak(format!("v{}", p.manifest.version));
                    ui.weak(p.manifest.kinds.join(", "));
                });
                if !p.manifest.description.is_empty() {
                    ui.weak(&p.manifest.description);
                }
                for perm in &p.manifest.permissions {
                    let granted = p.record.granted.iter().any(|g| g == perm);
                    ui.horizontal(|ui| {
                        if granted {
                            ui.weak(format!("✔ {perm}"));
                        } else {
                            ui.weak(format!("✖ {perm}"));
                            if ui.small_button("Grant").clicked() {
                                grant = Some((p.record.path.clone(), perm.clone()));
                            }
                        }
                    });
                }
                ui.separator();
            }
        });
    app.show_plugin_manager = open;
    if let Some(path) = install_path {
        let result = app.engine.plugins.write().install(&path, false);
        match result {
            Ok(m) => app.toast(format!("Installed plugin: {}", m.name), false),
            Err(e) => app.toast(format!("Install failed: {e}"), true),
        }
    }
    if let Some((path, enabled)) = toggle {
        app.engine.plugins.write().set_enabled(&path, enabled);
    }
    if let Some((path, perm)) = grant {
        app.engine.plugins.write().grant(&path, &perm);
    }
}

fn plugin_output(app: &mut ScApp, ctx: &egui::Context) {
    let Some((title, body)) = &app.plugin_output else { return };
    let title = title.clone();
    let body = body.clone();
    let mut open = true;
    egui::Window::new(title)
        .open(&mut open)
        .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
        .default_width(480.0)
        .show(ctx, |ui| {
            egui::ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
                ui.add(egui::Label::new(RichText::new(&body).monospace()));
            });
        });
    if !open {
        app.plugin_output = None;
    }
}

fn columns_dialog(app: &mut ScApp, ctx: &egui::Context) {
    if !app.show_columns {
        return;
    }
    // Ensure built-in + plugin columns are present in prefs.
    {
        let mut cols = app.settings.columns.clone();
        for (id, _label) in [
            ("index", "#"),
            ("name", "Name"),
            ("size", "Size"),
            ("type", "Type"),
            ("modified", "Modified"),
            ("created", "Created"),
            ("sha256", "SHA-256"),
        ] {
            if !cols.iter().any(|c| c.id == id) {
                let pref = crate::config::ColumnPref {
                    id: id.into(),
                    visible: id != "sha256",
                };
                if id == "index" {
                    cols.insert(0, pref);
                } else {
                    cols.push(pref);
                }
            }
        }
        let plugins: Vec<String> = {
            let host = app.engine.plugins.read();
            host.plugins
                .iter()
                .filter(|p| p.is_column())
                .map(|p| {
                    if p.manifest.column_title.is_empty() {
                        p.manifest.name.clone()
                    } else {
                        p.manifest.column_title.clone()
                    }
                })
                .collect()
        };
        for title in plugins {
            let id = format!("plugin:{title}");
            if !cols.iter().any(|c| c.id == id) {
                cols.push(crate::config::ColumnPref { id, visible: true });
            }
        }
        app.settings.columns = cols;
    }

    let mut open = app.show_columns;
    let mut persist = false;
    let mut move_up: Option<usize> = None;
    let mut move_down: Option<usize> = None;
    let mut reset = false;
    egui::Window::new("Columns")
        .open(&mut open)
        .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
        .default_width(360.0)
        .show(ctx, |ui| {
            ui.weak("Name is always shown. Drag order with Up/Down.");
            ui.add_space(6.0);
            let n = app.settings.columns.len();
            for i in 0..n {
                ui.horizontal(|ui| {
                    let is_name = app.settings.columns[i].id == "name";
                    if is_name {
                        let mut always = true;
                        ui.add_enabled(false, egui::Checkbox::new(&mut always, ""));
                    } else if ui
                        .checkbox(&mut app.settings.columns[i].visible, "")
                        .changed()
                    {
                        persist = true;
                    }
                    let id = app.settings.columns[i].id.clone();
                    let label = match id.as_str() {
                        "index" => "#".to_string(),
                        "name" => "Name".to_string(),
                        "size" => "Size".to_string(),
                        "type" => "Type".to_string(),
                        "modified" => "Modified".to_string(),
                        "created" => "Created".to_string(),
                        "sha256" => "SHA-256".to_string(),
                        id if id.starts_with("plugin:") => id["plugin:".len()..].to_string(),
                        other => other.to_string(),
                    };
                    ui.label(label);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.add_enabled(i + 1 < n, egui::Button::new("Down").small()).clicked() {
                            move_down = Some(i);
                        }
                        if ui.add_enabled(i > 0, egui::Button::new("Up").small()).clicked() {
                            move_up = Some(i);
                        }
                    });
                });
            }
            ui.add_space(8.0);
            if ui.button("Reset to defaults").clicked() {
                reset = true;
            }
        });
    if let Some(i) = move_up {
        app.settings.columns.swap(i, i - 1);
        persist = true;
    }
    if let Some(i) = move_down {
        app.settings.columns.swap(i, i + 1);
        persist = true;
    }
    if reset {
        app.settings.columns = crate::config::default_columns();
        persist = true;
    }
    if persist {
        app.persist_settings();
    }
    app.show_columns = open;
}

fn about(app: &mut ScApp, ctx: &egui::Context) {
    if !app.show_about {
        return;
    }
    let mut open = app.show_about;
    egui::Window::new("About")
        .open(&mut open)
        .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
        .resizable(false)
        .show(ctx, |ui| {
            ui.heading("SimpleCommander");
            ui.label(format!("Version {}", env!("CARGO_PKG_VERSION")));
            ui.add_space(4.0);
            ui.label("A fast dual-pane file explorer for Windows.");
            ui.weak("Rust + egui/wgpu · NTFS MFT instant search · WASM plugins");
        });
    app.show_about = open;
}

fn everything_prompt(app: &mut ScApp, ctx: &egui::Context) {
    if !app.everything_prompt_checked && app.start_time.elapsed().as_millis() > 1500 {
        app.everything_prompt_checked = true;
        if !app.settings.everything_prompt_dismissed
            && !sc_shell::everything::is_running()
            && !sc_shell::everything::is_installed()
        {
            app.show_everything_prompt = true;
        }
    }
    if !app.show_everything_prompt {
        return;
    }
    let mut open = true;
    let mut download = false;
    let mut not_now = false;
    let mut dismiss_forever = false;
    egui::Window::new("Install Everything?")
        .collapsible(false)
        .resizable(false)
        .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
        .open(&mut open)
        .order(egui::Order::Foreground)
        .show(ctx, |ui| {
            ui.label("Fast search uses voidtools Everything, which is not installed.");
            ui.add_space(4.0);
            ui.weak("Install Everything 1.5, then keep it running in the background.");
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                if ui.button(RichText::new("Download Everything").strong()).clicked() {
                    download = true;
                }
                if ui.button("Not now").clicked() {
                    not_now = true;
                }
                if ui.button("Don't ask again").clicked() {
                    dismiss_forever = true;
                }
            });
        });
    if download {
        sc_shell::everything::open_download_page();
        open = false;
    }
    if not_now {
        open = false;
    }
    if dismiss_forever {
        app.settings.everything_prompt_dismissed = true;
        app.persist_settings();
        open = false;
    }
    app.show_everything_prompt = open;
}

fn folder_compare(app: &mut ScApp, ctx: &egui::Context) {
    if !app.compare.open {
        return;
    }
    let mut open = true;
    let mut rerun = false;
    let mut copy_left = false;
    let mut copy_right = false;
    let mut delete_sel = false;
    egui::Window::new("Compare folders")
        .open(&mut open)
        .resizable(true)
        .default_size([720.0, 480.0])
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label(format!(
                    "Left: {}   Right: {}",
                    app.compare.left.display(),
                    app.compare.right.display()
                ));
            });
            ui.horizontal(|ui| {
                if ui
                    .checkbox(&mut app.compare.include_subfolders, "Include subfolders")
                    .changed()
                {
                    rerun = true;
                }
                ui.separator();
                let filter = app.compare.filter;
                for opt in [
                    crate::compare::CompareFilter::All,
                    crate::compare::CompareFilter::LeftOnly,
                    crate::compare::CompareFilter::RightOnly,
                    crate::compare::CompareFilter::Different,
                    crate::compare::CompareFilter::Same,
                ] {
                    if ui.selectable_label(filter == opt, opt.label()).clicked() {
                        app.compare.filter = opt;
                    }
                }
                if app.compare.running {
                    ui.spinner();
                    ui.weak("Comparing…");
                }
            });
            ui.separator();
            let filter = app.compare.filter;
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    let rows: Vec<(usize, String, String, bool)> = app
                        .compare
                        .rows
                        .iter()
                        .enumerate()
                        .filter(|(_, r)| filter.matches(r.kind))
                        .map(|(i, r)| {
                            (
                                i,
                                r.rel.clone(),
                                r.kind.label().to_string(),
                                app.compare.selected.contains(&i),
                            )
                        })
                        .collect();
                    for (i, rel, kind, sel) in rows {
                        ui.horizontal(|ui| {
                            let mut on = sel;
                            if ui.checkbox(&mut on, "").changed() {
                                if on {
                                    app.compare.selected.insert(i);
                                } else {
                                    app.compare.selected.remove(&i);
                                }
                            }
                            ui.monospace(kind);
                            ui.label(rel);
                        });
                    }
                });
            ui.separator();
            ui.horizontal(|ui| {
                if ui.button("Copy → right").clicked() {
                    copy_right = true;
                }
                if ui.button("Copy ← left").clicked() {
                    copy_left = true;
                }
                if ui.button("Delete selected").clicked() {
                    delete_sel = true;
                }
            });
        });
    if !open {
        app.compare.open = false;
        return;
    }
    if rerun {
        app.run_compare();
        return;
    }
    if copy_right || copy_left || delete_sel {
        let selected: Vec<usize> = app.compare.selected.iter().copied().collect();
        for i in selected {
            let Some(row) = app.compare.rows.get(i) else { continue };
            if row.is_dir {
                continue;
            }
            if copy_right {
                if let Some(src) = &row.left {
                    app.submit_op(Operation::Copy {
                        sources: vec![src.clone()],
                        dest_dir: app.compare.right.clone(),
                    });
                }
            } else if copy_left {
                if let Some(src) = &row.right {
                    app.submit_op(Operation::Copy {
                        sources: vec![src.clone()],
                        dest_dir: app.compare.left.clone(),
                    });
                }
            } else if delete_sel {
                let mut paths = Vec::new();
                if let Some(p) = &row.left {
                    paths.push(p.clone());
                }
                if let Some(p) = &row.right {
                    paths.push(p.clone());
                }
                if !paths.is_empty() {
                    app.submit_op(Operation::Delete {
                        paths,
                        recycle: true,
                    });
                }
            }
        }
        app.compare.selected.clear();
    }
}

