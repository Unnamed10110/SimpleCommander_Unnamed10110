//! Navigation sidebar: favorites/catalog, drives, known folders, and a lazy
//! folder tree fed by background jobs (the UI thread never lists directories).

use crate::app::ScApp;
use crate::jobs::Job;
use egui::{RichText, Ui};
use sc_core::entry::format_size;
use std::path::PathBuf;

pub fn draw(app: &mut ScApp, ui: &mut Ui) {
    app.maybe_refresh_places();
    let inner = egui::Panel::left("sidebar")
        .resizable(true)
        .default_size(app.sidebar_width)
        .min_size(140.0)
        .max_size(480.0)
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    favorites_section(app, ui);
                    drives_section(app, ui);
                    known_folders_section(app, ui);
                    recycle_row(app, ui);
                    wsl_section(app, ui);
                    network_section(app, ui);
                    tree_section(app, ui);
                });
        });
    let w = inner.response.rect.width();
    if (w - app.sidebar_width).abs() > 0.5 {
        app.sidebar_width = w.clamp(140.0, 480.0);
    }
}

fn favorites_section(app: &mut ScApp, ui: &mut Ui) {
    let mut open = app.settings.session.sidebar_favorites_open;
    let mut remove: Option<usize> = None;
    let mut go: Option<PathBuf> = None;
    let mut open_tab: Option<PathBuf> = None;
    let resp = egui::CollapsingHeader::new(RichText::new("Favorites").strong())
        .open(Some(open))
        .show(ui, |ui| {
            for (i, fav) in app.settings.session.favorites.iter().enumerate() {
                let name = fav
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| fav.display().to_string());
                let resp = ui.selectable_label(false, format!("★ {name}"));
                if resp.clicked() {
                    go = Some(fav.clone());
                }
                if resp.middle_clicked() {
                    open_tab = Some(fav.clone());
                }
                resp.on_hover_text(fav.display().to_string()).context_menu(|ui| {
                    if ui.button("Remove favorite").clicked() {
                        remove = Some(i);
                        ui.close();
                    }
                });
            }
            if ui.small_button("+ Add current folder").clicked() {
                let path = app.active_tab().path.clone();
                if !app.is_favorite(&path) {
                    app.toggle_favorite(path);
                }
            }
        });
    if resp.header_response.clicked() {
        open = !open;
    }
    app.settings.session.sidebar_favorites_open = open;
    if let Some(i) = remove {
        app.settings.session.favorites.remove(i);
        app.persist_settings();
    }
    if let Some(p) = open_tab {
        let pane = app.active_pane;
        app.open_folder_in_new_tab(pane, p);
    } else if let Some(p) = go {
        let pane = app.active_pane;
        app.navigate(pane, p);
    }
}

fn drives_section(app: &mut ScApp, ui: &mut Ui) {
    let mut open = app.settings.session.sidebar_drives_open;
    let mut go: Option<PathBuf> = None;
    let mut open_tab: Option<PathBuf> = None;
    let resp = egui::CollapsingHeader::new(RichText::new("Drives").strong())
        .open(Some(open))
        .show(ui, |ui| {
            for v in &app.volumes {
                let icon = match v.drive_type {
                    sc_shell::volumes::DriveType::Removable => "🔌",
                    sc_shell::volumes::DriveType::Network => "🌐",
                    sc_shell::volumes::DriveType::CdRom => "💿",
                    _ => "💾",
                };
                let label = if v.label.is_empty() {
                    format!("{icon} {}", v.root.display())
                } else {
                    format!("{icon} {} ({})", v.root.display(), v.label)
                };
                let resp = ui.selectable_label(false, label);
                if resp.clicked() {
                    go = Some(v.root.clone());
                }
                if resp.middle_clicked() {
                    open_tab = Some(v.root.clone());
                }
                if v.total_bytes > 0 {
                    resp.on_hover_text(format!(
                        "{} free of {}",
                        format_size(v.free_bytes),
                        format_size(v.total_bytes)
                    ));
                }
            }
        });
    if resp.header_response.clicked() {
        open = !open;
    }
    app.settings.session.sidebar_drives_open = open;
    if let Some(p) = open_tab {
        let pane = app.active_pane;
        app.open_folder_in_new_tab(pane, p);
    } else if let Some(p) = go {
        let pane = app.active_pane;
        app.navigate(pane, p);
    }
}

fn known_folders_section(app: &mut ScApp, ui: &mut Ui) {
    let mut open = app.settings.session.sidebar_user_folders_open;
    let mut go: Option<PathBuf> = None;
    let mut open_tab: Option<PathBuf> = None;
    let resp = egui::CollapsingHeader::new(RichText::new("User folders").strong())
        .open(Some(open))
        .show(ui, |ui| {
            for (name, path) in app.known_folders.clone() {
                let resp = ui.selectable_label(false, format!("📁 {name}"));
                if resp.clicked() {
                    go = Some(path.clone());
                }
                if resp.middle_clicked() {
                    open_tab = Some(path);
                }
            }
        });
    if resp.header_response.clicked() {
        open = !open;
    }
    app.settings.session.sidebar_user_folders_open = open;
    if let Some(p) = open_tab {
        let pane = app.active_pane;
        app.open_folder_in_new_tab(pane, p);
    } else if let Some(p) = go {
        let pane = app.active_pane;
        app.navigate(pane, p);
    }
}

fn recycle_row(app: &mut ScApp, ui: &mut Ui) {
    let resp = ui.selectable_label(false, "🗑 Recycle Bin");
    if resp.clicked() {
        let pane = app.active_pane;
        app.navigate(pane, sc_shell::recycle::recycle_root());
    }
    if resp.middle_clicked() {
        let pane = app.active_pane;
        app.open_folder_in_new_tab(pane, sc_shell::recycle::recycle_root());
    }
}

fn wsl_section(app: &mut ScApp, ui: &mut Ui) {
    if app.wsl_distros.is_empty() {
        return;
    }
    let distros = app.wsl_distros.clone();
    let mut open = app.settings.session.sidebar_wsl_open;
    let mut go: Option<PathBuf> = None;
    let mut open_tab: Option<PathBuf> = None;
    let resp = egui::CollapsingHeader::new(RichText::new("WSL").strong())
        .open(Some(open))
        .show(ui, |ui| {
            for (name, path) in &distros {
                let r = ui.selectable_label(false, format!("🐧 {name}"));
                if r.clicked() {
                    go = Some(path.clone());
                }
                if r.middle_clicked() {
                    open_tab = Some(path.clone());
                }
            }
        });
    if resp.header_response.clicked() {
        open = !open;
    }
    app.settings.session.sidebar_wsl_open = open;
    if let Some(p) = open_tab {
        let pane = app.active_pane;
        app.open_folder_in_new_tab(pane, p);
    } else if let Some(p) = go {
        let pane = app.active_pane;
        app.navigate(pane, p);
    }
}

fn network_section(app: &mut ScApp, ui: &mut Ui) {
    let mut places = app.network_places.clone();
    for p in &app.settings.session.unc_roots {
        if !places.iter().any(|(_, q)| q == p) {
            let label = p.display().to_string();
            places.push((label, p.clone()));
        }
    }
    let mut open = app.settings.session.sidebar_network_open;
    let mut go: Option<PathBuf> = None;
    let mut open_tab: Option<PathBuf> = None;
    let resp = egui::CollapsingHeader::new(RichText::new("Network").strong())
        .open(Some(open))
        .show(ui, |ui| {
            if places.is_empty() {
                ui.weak("No shares");
            }
            for (name, path) in &places {
                let r = ui.selectable_label(false, format!("🌐 {name}"));
                if r.clicked() {
                    go = Some(path.clone());
                }
                if r.middle_clicked() {
                    open_tab = Some(path.clone());
                }
                r.on_hover_text(path.display().to_string());
            }
        });
    if resp.header_response.clicked() {
        open = !open;
    }
    app.settings.session.sidebar_network_open = open;
    if let Some(p) = open_tab {
        let pane = app.active_pane;
        app.open_folder_in_new_tab(pane, p);
    } else if let Some(p) = go {
        let pane = app.active_pane;
        app.navigate(pane, p);
    }
}

fn tree_section(app: &mut ScApp, ui: &mut Ui) {
    let mut open = app.settings.session.sidebar_tree_open;
    let mut go: Option<PathBuf> = None;
    let mut open_tab: Option<PathBuf> = None;
    let resp = egui::CollapsingHeader::new(RichText::new("Tree").strong())
        .open(Some(open))
        .show(ui, |ui| {
            let roots: Vec<PathBuf> = app.volumes.iter().map(|v| v.root.clone()).collect();
            for root in roots {
                tree_node(app, ui, &root, &mut go, &mut open_tab, 0);
            }
        });
    if resp.header_response.clicked() {
        open = !open;
    }
    app.settings.session.sidebar_tree_open = open;
    if let Some(p) = open_tab {
        let pane = app.active_pane;
        app.open_folder_in_new_tab(pane, p);
    } else if let Some(p) = go {
        let pane = app.active_pane;
        app.navigate(pane, p);
    }
}

fn tree_node(
    app: &mut ScApp,
    ui: &mut Ui,
    path: &PathBuf,
    go: &mut Option<PathBuf>,
    open_tab: &mut Option<PathBuf>,
    depth: usize,
) {
    if depth > 24 {
        return;
    }
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string());
    let node_open = app.tree_open.contains(path);
    ui.horizontal(|ui| {
        let toggle = ui.small_button(if node_open { "▾" } else { "▸" });
        if toggle.clicked() {
            if node_open {
                app.tree_open.remove(path);
            } else {
                app.tree_open.insert(path.clone());
                if !app.tree_children.contains_key(path) && !app.tree_pending.contains(path) {
                    app.tree_pending.insert(path.clone());
                    app.engine.submit(Job::ListDirs { path: path.clone() });
                }
            }
        }
        let is_current = app.active_tab().path == *path;
        let resp = ui.selectable_label(is_current, format!("📁 {name}"));
        if resp.clicked() {
            *go = Some(path.clone());
        }
        if resp.middle_clicked() {
            *open_tab = Some(path.clone());
        }
    });
    if node_open {
        ui.indent(("tree", path), |ui| {
            match app.tree_children.get(path) {
                Some(children) => {
                    let children = children.clone();
                    for child in children {
                        let child_path = path.join(&child);
                        tree_node(app, ui, &child_path, go, open_tab, depth + 1);
                    }
                }
                None => {
                    ui.weak("…");
                }
            }
        });
    }
}
