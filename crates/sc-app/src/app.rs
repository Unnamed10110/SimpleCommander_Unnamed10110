//! Application state and event handling. UI rendering lives in `ui`,
//! `dialogs`, and `sidebar`; this module owns state transitions and the
//! async result plumbing.

use crate::config::{self, ConflictDefault, Settings};
use crate::jobs::{Job, JobEngine, ListingToken, PreviewContent, UiMsg};
use crate::keymap::ShortcutId;
use crate::preview::{AudioCtl, AudioPreview, HexPreview, WebEmbed};
use crate::tags::TagStore;
use crate::theme::{self, Theme};
use egui::TextureHandle;
use sc_core::snapshot::DirSnapshot;
use sc_core::sort::{SortKey, SortSpec};
use sc_core::state::{PaneLayout, PaneState, Session, SessionPane, SessionTab, SplitDirection, TabState};
use sc_ops::queue::{ConflictResolution, OpEngine, OpEvent, OpOrigin, Operation};
use sc_ops::undo::UndoJournal;
use sc_shell::recycle::RecycleItem;
use sc_shell::watcher::DirWatcher;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

pub struct IconCache {
    pub map: HashMap<String, Option<TextureHandle>>,
    pub pending: HashSet<String>,
}

impl IconCache {
    fn new() -> Self {
        Self { map: HashMap::new(), pending: HashSet::new() }
    }
}

/// Inline rename in progress.
pub struct RenameState {
    pub pane: usize,
    pub tab_uid: u64,
    pub entry_index: u32,
    pub buffer: String,
    pub focus_requested: bool,
}

/// Inline tab-title edit.
pub struct TabRename {
    pub pane: usize,
    pub index: usize,
    pub buffer: String,
    pub focus_requested: bool,
}

/// A running/finished operation shown in the transfer queue panel.
pub struct OpView {
    pub op_id: u64,
    pub label: String,
    pub done_bytes: u64,
    pub total_bytes: u64,
    pub done_files: u64,
    pub total_files: u64,
    pub current: String,
    pub finished: bool,
    pub error: Option<String>,
    pub started: Instant,
}

pub struct ConflictPrompt {
    pub op_id: u64,
    pub source: PathBuf,
    pub dest: PathBuf,
    pub apply_to_all: bool,
}

#[derive(PartialEq, Clone, Copy)]
pub enum SearchMode {
    NameHere,
    NameGlobal,
    Content,
}

impl SearchMode {
    pub fn next(self) -> Self {
        match self {
            Self::NameHere => Self::NameGlobal,
            Self::NameGlobal => Self::Content,
            Self::Content => Self::NameHere,
        }
    }

    pub fn prev(self) -> Self {
        match self {
            Self::NameHere => Self::Content,
            Self::Content => Self::NameGlobal,
            Self::NameGlobal => Self::NameHere,
        }
    }
}

pub struct SearchState {
    pub open: bool,
    pub query: String,
    pub mode: SearchMode,
    pub results: Vec<(PathBuf, bool)>,
    pub query_id: u64,
    pub running: bool,
    pub focus_requested: bool,
    pub selected: usize,
}

pub struct PaletteState {
    pub open: bool,
    pub query: String,
    pub results: Vec<(PathBuf, bool)>,
    pub query_id: u64,
    pub selected: usize,
    pub focus_requested: bool,
}

pub struct BatchRenameState {
    pub open: bool,
    pub pane: usize,
    pub items: Vec<PathBuf>,
    pub pattern: String,
    pub find: String,
    pub replace: String,
    pub use_regex: bool,
    pub case: usize, // 0 keep, 1 lower, 2 upper, 3 title
    pub counter_start: u32,
    pub error: Option<String>,
}

pub struct TagEditState {
    pub open: bool,
    pub path: PathBuf,
    pub tags: String,
    pub comment: String,
}

pub struct PreviewState {
    pub enabled: bool,
    pub path: Option<PathBuf>,
    pub generation: u64,
    pub texture: Option<TextureHandle>,
    pub text: Option<String>,
    pub info: Option<String>,
    pub hex: Option<HexPreview>,
    pub audio: Option<AudioPreview>,
    pub web_url: Option<String>,
    pub web_fallback: Option<String>,
    pub webview: Option<WebEmbed>,
    pub webview_error: Option<String>,
    pub embed_rect: Option<egui::Rect>,
    pub parent_hwnd: Option<isize>,
    pub audio_ctl: AudioCtl,
    pub loading: bool,
    /// Space is still held from the shortcut that opened preview.
    pub space_armed: bool,
    pub prev_space_down: bool,
    pub prev_esc_down: bool,
}

pub struct AddressEdit {
    pub pane: usize,
    pub buffer: String,
    pub focus_requested: bool,
}

/// Name prompt shown before creating a file or folder.
pub struct NewItemPrompt {
    pub is_folder: bool,
    pub pane: usize,
    pub name: String,
    pub focus_requested: bool,
    pub error: Option<String>,
}

pub struct ScApp {
    pub theme: Theme,
    pub layout: PaneLayout,
    pub panes: Vec<PaneState>,
    pub active_pane: usize,
    pub show_hidden: bool,
    pub settings: Settings,

    pub engine: JobEngine,
    pub ops: OpEngine,
    pub undo: UndoJournal,
    pub icons: IconCache,
    pub tags: TagStore,

    pub watchers: HashMap<(usize, u64), DirWatcher>,
    pub keep_selection: HashMap<u64, Vec<String>>,
    pub folder_sizes: HashMap<PathBuf, u64>,
    pub folder_size_pending: HashSet<PathBuf>,

    pub ops_view: Vec<OpView>,
    pub ops_selected: Option<u64>,
    pub conflict: Option<ConflictPrompt>,
    pub conflict_queue: VecDeque<ConflictPrompt>,
    pub pending_ops: HashMap<u64, Operation>,
    pub recycle_meta: HashMap<String, RecycleItem>,
    pub compare: crate::compare::FolderCompareState,
    pub pending_delete: Option<(Vec<PathBuf>, bool)>, // (paths, permanent)
    pub rename: Option<RenameState>,
    pub search: SearchState,
    pub palette: PaletteState,
    pub batch_rename: BatchRenameState,
    pub tag_edit: Option<TagEditState>,
    pub preview: PreviewState,
    pub address_edit: Option<AddressEdit>,
    pub filter_focus: Option<usize>,
    pub show_plugin_manager: bool,
    pub show_about: bool,
    pub show_everything_prompt: bool,
    pub everything_prompt_checked: bool,
    pub show_settings: bool,
    pub settings_cat: usize,
    pub show_columns: bool,
    pub tab_rename: Option<TabRename>,
    pub plugin_output: Option<(String, String)>, // (title, body)
    pub toasts: Vec<(String, Instant, bool)>,    // (msg, when, is_error)

    pub column_values: HashMap<(usize, PathBuf), Option<String>>,
    pub column_pending: HashSet<(usize, PathBuf)>,
    pub checksums: HashMap<PathBuf, Option<String>>,
    pub checksum_pending: HashSet<PathBuf>,

    pub type_ahead: String,
    pub type_ahead_at: Instant,
    pub last_session_save: Instant,
    pub start_time: Instant,
    /// Time from process start to the first rendered frame, captured once.
    pub startup_ms: Option<u64>,
    pub volumes: Vec<sc_shell::volumes::VolumeInfo>,
    pub volumes_refreshed: Instant,
    pub tree_children: HashMap<PathBuf, Vec<String>>,
    pub tree_open: HashSet<PathBuf>,
    pub tree_pending: HashSet<PathBuf>,
    /// Screen rects of the panes from the last frame (for drop targeting).
    pub pane_rects: Vec<egui::Rect>,
    /// Guard so only one OLE drag starts per gesture.
    pub drag_active: bool,
    /// Fraction of the dual split occupied by pane 0.
    pub split_ratio: f32,
    /// Width of the navigation sidebar in points.
    pub sidebar_width: f32,
    pub preview_width: f32,
    pub preview_height: f32,
    /// Right-click on empty pane space: (pane, pointer pos).
    pub pane_bg_menu: Option<(usize, egui::Pos2)>,
    /// Right-click on a file row: (pane, entry index, pointer pos).
    pub row_ctx_menu: Option<(usize, u32, egui::Pos2)>,
    /// Settings row currently waiting for a key press.
    pub capture_shortcut: Option<ShortcutId>,
    /// After New file/folder completes, select this name in the list.
    /// After New file/folder or a copy/move into a tab, select these names.
    pub pending_select: Option<(u64, Vec<String>)>,
    /// Scroll the file table to the cursor for this tab once.
    pub force_scroll_tab: Option<u64>,
    /// Rubber-band selection in a file list (drag on empty space or unselected rows).
    pub marquee: Option<Marquee>,
    /// Name prompt for New file / New folder.
    pub new_item: Option<NewItemPrompt>,
}

/// In-progress mouse region selection.
#[derive(Clone)]
pub struct Marquee {
    pub pane: usize,
    pub tab_uid: u64,
    pub origin: egui::Pos2,
    /// Ctrl/Cmd held at drag start → union with the previous selection.
    pub additive: bool,
    pub keep: HashSet<u32>,
}

impl ScApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let settings = config::load_settings();
        sc_shell::everything::set_preferred_exe(
            (!settings.everything_exe.trim().is_empty())
                .then(|| PathBuf::from(settings.everything_exe.trim())),
        );
        let session = if settings.restore_session {
            settings.session.clone()
        } else {
            let mut s = Session::default();
            s.theme = settings.session.theme.clone();
            s.favorites = settings.session.favorites.clone();
            s.show_hidden = settings.session.show_hidden;
            s.layout = settings.default_layout.to_pane_layout();
            s
        };
        theme::install_font_fallbacks(&cc.egui_ctx);
        let mut theme = theme::by_name(&session.theme);
        theme::apply_accent_override(&mut theme, &settings.accent);
        theme::apply(&cc.egui_ctx, &theme);
        cc.egui_ctx.set_zoom_factor(settings.ui_scale.clamp(0.75, 2.0));

        let plugins = Arc::new(parking_lot::RwLock::new(
            sc_plugins::host::PluginHost::new(config::plugins_registry_path())
                .expect("plugin host"),
        ));
        let engine = JobEngine::new(cc.egui_ctx.clone(), plugins, settings.index_enabled);

        let ctx = cc.egui_ctx.clone();
        let ops = OpEngine::new(move || ctx.request_repaint());
        ops.set_max_jobs(settings.transfer_jobs.clamp(1, 4) as usize);
        let hook_plugins = engine.plugins.clone();
        ops.set_hooks(Arc::new(move |event, sources, dests| {
            hook_plugins.read().run_hooks(event, sources, dests);
        }));

        let mut panes: Vec<PaneState> = session
            .panes
            .iter()
            .map(|sp| {
                let mut pane = PaneState::new(
                    sp.tabs
                        .first()
                        .map(|t| t.path.clone())
                        .unwrap_or_else(sc_core::state::dirs_home),
                );
                pane.tabs.clear();
                for st in &sp.tabs {
                    let mut tab = TabState::new(st.path.clone());
                    tab.locked = st.locked;
                    tab.custom_title = st.custom_title.clone();
                    tab.color = st.color.clone();
                    if settings.remember_sort {
                        tab.sort = st.sort;
                    }
                    pane.tabs.push(tab);
                }
                if pane.tabs.is_empty() {
                    pane.tabs.push(TabState::new(sc_core::state::dirs_home()));
                }
                pane.active_tab = sp.active_tab.min(pane.tabs.len() - 1);
                pane
            })
            .collect();
        while panes.len() < 2 {
            panes.push(PaneState::new(sc_core::state::dirs_home()));
        }

        let mut app = Self {
            theme,
            layout: session.layout,
            panes,
            active_pane: session.active_pane.min(1),
            show_hidden: session.show_hidden,
            settings,
            engine,
            ops,
            undo: UndoJournal::default(),
            icons: IconCache::new(),
            tags: TagStore::open(&config::tags_db_path()),
            watchers: HashMap::new(),
            keep_selection: HashMap::new(),
            folder_sizes: HashMap::new(),
            folder_size_pending: HashSet::new(),
            ops_view: Vec::new(),
            ops_selected: None,
            conflict: None,
            conflict_queue: VecDeque::new(),
            pending_ops: HashMap::new(),
            recycle_meta: HashMap::new(),
            compare: crate::compare::FolderCompareState::default(),
            pending_delete: None,
            rename: None,
            search: SearchState {
                open: false,
                query: String::new(),
                mode: SearchMode::NameHere,
                results: Vec::new(),
                query_id: 0,
                running: false,
                focus_requested: false,
                selected: 0,
            },
            palette: PaletteState {
                open: false,
                query: String::new(),
                results: Vec::new(),
                query_id: 0,
                selected: 0,
                focus_requested: false,
            },
            batch_rename: BatchRenameState {
                open: false,
                pane: 0,
                items: Vec::new(),
                pattern: "<name>".into(),
                find: String::new(),
                replace: String::new(),
                use_regex: false,
                case: 0,
                counter_start: 1,
                error: None,
            },
            tag_edit: None,
            preview: PreviewState {
                enabled: false,
                path: None,
                generation: 0,
                texture: None,
                text: None,
                info: None,
                hex: None,
                audio: None,
                web_url: None,
                web_fallback: None,
                webview: None,
                webview_error: None,
                embed_rect: None,
                parent_hwnd: None,
                audio_ctl: AudioCtl::default(),
                loading: false,
                space_armed: false,
                prev_space_down: false,
                prev_esc_down: false,
            },
            address_edit: None,
            filter_focus: None,
            show_plugin_manager: false,
            show_about: false,
            show_everything_prompt: false,
            everything_prompt_checked: false,
            show_settings: false,
            settings_cat: 0,
            show_columns: false,
            tab_rename: None,
            plugin_output: None,
            toasts: Vec::new(),
            column_values: HashMap::new(),
            column_pending: HashSet::new(),
            checksums: HashMap::new(),
            checksum_pending: HashSet::new(),
            type_ahead: String::new(),
            type_ahead_at: Instant::now(),
            last_session_save: Instant::now(),
            start_time: Instant::now(),
            startup_ms: None,
            volumes: sc_shell::volumes::list_volumes(),
            volumes_refreshed: Instant::now(),
            tree_children: HashMap::new(),
            tree_open: HashSet::new(),
            tree_pending: HashSet::new(),
            pane_rects: Vec::new(),
            drag_active: false,
            split_ratio: session.split_ratio.clamp(0.15, 0.85),
            sidebar_width: session.sidebar_width.clamp(140.0, 480.0),
            preview_width: session.preview_width.clamp(200.0, 800.0),
            preview_height: session.preview_height.clamp(140.0, 600.0),
            pane_bg_menu: None,
            row_ctx_menu: None,
            capture_shortcut: None,
            pending_select: None,
            force_scroll_tab: None,
            marquee: None,
            new_item: None,
        };
        app.tree_open = session.tree_expanded.iter().cloned().collect();
        for path in app.tree_open.clone() {
            if !app.tree_pending.contains(&path) {
                app.tree_pending.insert(path.clone());
                app.engine.submit(Job::ListDirs { path });
            }
        }
        for pane in 0..app.panes.len() {
            for tab in 0..app.panes[pane].tabs.len() {
                app.request_listing_for(pane, tab, false);
            }
        }
        app
    }

    // ----- listing plumbing ---------------------------------------------

    pub fn active_tab(&self) -> &TabState {
        self.panes[self.active_pane].tab()
    }

    /// Kick off (re-)listing of a pane's active tab.
    pub fn request_listing(&mut self, pane: usize, keep_selection: bool) {
        let tab_index = self.panes[pane].active_tab;
        self.request_listing_for(pane, tab_index, keep_selection);
    }

    pub fn request_listing_for(&mut self, pane: usize, tab_index: usize, keep_selection: bool) {
        let tab = &mut self.panes[pane].tabs[tab_index];
        tab.generation += 1;
        tab.pending.clear();
        tab.loading = true;
        let token = ListingToken { pane, tab_uid: tab.uid, generation: tab.generation };
        let path = tab.path.clone();
        let flatten = tab.flatten;
        if keep_selection {
            self.keep_selection.insert(tab.uid, tab.selected_names());
        } else {
            self.keep_selection.remove(&tab.uid);
        }
        self.engine.submit(Job::ReadDir { token, path: path.clone(), flatten });
        self.ensure_watcher(pane, tab_index);
        self.tags.invalidate(&path);
    }

    fn ensure_watcher(&mut self, pane: usize, tab_index: usize) {
        let tab = &self.panes[pane].tabs[tab_index];
        let key = (pane, tab.uid);
        let path = tab.path.clone();
        let subtree = tab.flatten;
        // No watcher inside archives.
        if crate::vfs::zip_listing(&path).is_some() || sc_shell::recycle::is_recycle_path(&path) {
            self.watchers.remove(&key);
            return;
        }
        if self
            .watchers
            .get(&key)
            .is_some_and(|w| w.watches(&path, subtree))
        {
            return;
        }
        let uid = tab.uid;
        let tx = self.engine.results_tx.clone();
        let ctx_pane = pane;
        if let Some(w) = DirWatcher::spawn(&path, uid, subtree, move |id| {
            let _ = tx.send(UiMsg::DirChanged { pane: ctx_pane, tab_uid: id });
        }) {
            self.watchers.insert(key, w);
        } else {
            self.watchers.remove(&key);
        }
    }

    pub fn navigate(&mut self, pane: usize, to: PathBuf) {
        self.remember_unc(&to);
        let tab = self.panes[pane].tab_mut();
        if tab.locked && tab.path != to {
            // Locked tabs open navigation in a new tab (XYplorer behavior).
            self.panes[pane].add_tab(to);
            let tab_index = self.panes[pane].active_tab;
            self.request_listing_for(pane, tab_index, false);
            return;
        }
        tab.navigate(to);
        self.request_listing(pane, false);
    }

    fn remember_unc(&mut self, path: &Path) {
        let s = path.to_string_lossy();
        if !s.starts_with("\\\\") {
            return;
        }
        // Keep the share root (`\\server\share`).
        let mut comps = path.components();
        let prefix = comps.next();
        let server = comps.next();
        let share = comps.next();
        let root = match (prefix, server, share) {
            (Some(_), Some(srv), Some(sh)) => {
                PathBuf::from(format!(r"\\{}\{}", srv.as_os_str().to_string_lossy(), sh.as_os_str().to_string_lossy()))
            }
            _ => path.to_path_buf(),
        };
        let roots = &mut self.settings.session.unc_roots;
        if !roots.iter().any(|p| p == &root) {
            roots.push(root);
            if roots.len() > 24 {
                roots.remove(0);
            }
        }
    }

    pub fn submit_op(&mut self, op: Operation) {
        self.submit_op_origin(op, OpOrigin::User);
    }

    pub fn submit_op_origin(&mut self, op: Operation, origin: OpOrigin) {
        let id = self.ops.submit_origin(op.clone(), origin);
        self.pending_ops.insert(id, op);
    }

    pub fn undo(&mut self) {
        for op in self.undo.pop_undo() {
            self.submit_op_origin(op, OpOrigin::Undo);
        }
    }

    pub fn redo(&mut self) {
        for op in self.undo.pop_redo() {
            self.submit_op_origin(op, OpOrigin::Redo);
        }
    }

    pub fn open_compare(&mut self) {
        if !matches!(self.layout, PaneLayout::Dual(_)) {
            self.toast("Compare folders needs a dual-pane layout".into(), true);
            return;
        }
        let left = self.panes[0].tab().path.clone();
        let right = self.panes[1].tab().path.clone();
        if sc_shell::recycle::is_recycle_path(&left) || sc_shell::recycle::is_recycle_path(&right) {
            self.toast("Cannot compare the Recycle Bin".into(), true);
            return;
        }
        self.compare.open = true;
        self.compare.left = left;
        self.compare.right = right;
        self.compare.rows.clear();
        self.compare.selected.clear();
        self.run_compare();
    }

    pub fn run_compare(&mut self) {
        self.compare.query_id = self.compare.query_id.wrapping_add(1);
        self.compare.running = true;
        self.engine.submit(Job::CompareFolders {
            query_id: self.compare.query_id,
            left: self.compare.left.clone(),
            right: self.compare.right.clone(),
            recursive: self.compare.include_subfolders,
        });
    }

    pub fn open_folder_in_new_tab(&mut self, pane: usize, path: PathBuf) {
        let tab_index = self.panes[pane].insert_tab_beside(path);
        self.request_listing_for(pane, tab_index, false);
    }

    /// Open the New folder name prompt for this pane.
    pub fn begin_new_folder(&mut self, pane: usize) {
        self.begin_new_item(pane, true);
    }

    /// Open the New file name prompt for this pane.
    pub fn begin_new_file(&mut self, pane: usize) {
        self.begin_new_item(pane, false);
    }

    fn begin_new_item(&mut self, pane: usize, is_folder: bool) {
        let tab = self.panes[pane].tab();
        if crate::vfs::split_zip_path(&tab.path).is_some() || sc_shell::recycle::is_recycle_path(&tab.path) {
            self.toast(
                if sc_shell::recycle::is_recycle_path(&tab.path) {
                    "Can't create items in the Recycle Bin".into()
                } else if is_folder {
                    "Can't create a folder inside a zip".into()
                } else {
                    "Can't create a file inside a zip".into()
                },
                true,
            );
            return;
        }
        let seed = if is_folder { "New folder" } else { "New file.txt" };
        let dest = sc_ops::queue::auto_rename(&tab.path.join(seed));
        let name = dest
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| seed.into());
        self.active_pane = pane;
        self.new_item = Some(NewItemPrompt {
            is_folder,
            pane,
            name,
            focus_requested: true,
            error: None,
        });
    }

    /// Create the file/folder named in the prompt, then select it in the list.
    pub fn submit_new_item(&mut self) {
        let Some(prompt) = self.new_item.as_ref() else {
            return;
        };
        let pane = prompt.pane;
        let is_folder = prompt.is_folder;
        let name = prompt.name.trim().to_string();
        if let Some(err) = invalid_item_name(&name) {
            if let Some(p) = self.new_item.as_mut() {
                p.error = Some(err.to_string());
            }
            return;
        }
        let tab = self.panes[pane].tab();
        if crate::vfs::split_zip_path(&tab.path).is_some() {
            self.new_item = None;
            self.toast("Can't create items inside a zip".into(), true);
            return;
        }
        let dest = tab.path.join(&name);
        if dest.exists() {
            if let Some(p) = self.new_item.as_mut() {
                p.error = Some("An item with that name already exists".into());
            }
            return;
        }
        let uid = tab.uid;
        self.pending_select = Some((uid, vec![name]));
        self.force_scroll_tab = Some(uid);
        self.new_item = None;
        if is_folder {
            self.submit_op(Operation::NewFolder { path: dest });
        } else {
            self.submit_op(Operation::NewFile { path: dest });
        }
    }

    pub fn is_favorite(&self, path: &Path) -> bool {
        self.settings.session.favorites.iter().any(|p| p == path)
    }

    pub fn toggle_favorite(&mut self, path: PathBuf) {
        let favs = &mut self.settings.session.favorites;
        if let Some(i) = favs.iter().position(|p| *p == path) {
            favs.remove(i);
        } else {
            favs.push(path);
        }
        self.persist_settings();
    }

    /// Launch the configured terminal in the pane's current folder.
    pub fn open_terminal(&mut self, pane: usize) {
        let folder = self.panes[pane].tab().path.clone();
        if crate::vfs::split_zip_path(&folder).is_some() {
            self.toast("Can't open a terminal inside a zip".into(), true);
            return;
        }
        let filled = self
            .settings
            .terminal_command
            .replace("{path}", &folder.display().to_string());
        let parts = split_cmdline(&filled);
        if parts.is_empty() {
            self.toast("Terminal command is empty".into(), true);
            return;
        }
        let mut cmd = std::process::Command::new(&parts[0]);
        if parts.len() > 1 {
            cmd.args(&parts[1..]);
        }
        cmd.current_dir(&folder);
        match cmd.spawn() {
            Ok(_) => {}
            Err(e) => self.toast(format!("Terminal: {e}"), true),
        }
    }

    pub fn refresh_all_matching(&mut self, dirs: &[PathBuf]) {
        for pane in 0..self.panes.len() {
            for tab_index in 0..self.panes[pane].tabs.len() {
                let tab_path = self.panes[pane].tabs[tab_index].path.clone();
                if dirs.iter().any(|d| *d == tab_path) {
                    self.request_listing_for(pane, tab_index, true);
                }
            }
        }
    }

    /// Rebuild the sorted/filtered view for a tab (async).
    pub fn rebuild_view(&mut self, pane: usize) {
        let show_hidden = self.show_hidden;
        let tab = self.panes[pane].tab_mut();
        if tab.snapshot.entries.is_empty() {
            tab.view = Arc::new(Vec::new());
            return;
        }
        let token = ListingToken { pane, tab_uid: tab.uid, generation: tab.generation };
        self.engine.submit(Job::BuildView {
            token,
            entries: tab.snapshot.entries.clone(),
            spec: tab.sort,
            filter: tab.filter.clone(),
            show_hidden,
        });
    }

    // ----- async message pump -------------------------------------------

    pub fn pump_messages(&mut self, ctx: &egui::Context) {
        // Bounded per frame to keep frame time stable even under floods.
        for _ in 0..256 {
            let Ok(msg) = self.engine.results.try_recv() else { break };
            self.handle_msg(ctx, msg);
        }
        for _ in 0..64 {
            let Ok(ev) = self.ops.events.try_recv() else { break };
            self.handle_op_event(ev);
        }
        self.toasts
            .retain(|(_, when, _)| when.elapsed().as_secs_f32() < 6.0);
    }

    fn find_tab(&mut self, pane: usize, tab_uid: u64) -> Option<(usize, usize)> {
        if pane < self.panes.len() {
            if let Some(i) = self.panes[pane].tabs.iter().position(|t| t.uid == tab_uid) {
                return Some((pane, i));
            }
        }
        // Tab may have moved panes; search everywhere.
        for p in 0..self.panes.len() {
            if let Some(i) = self.panes[p].tabs.iter().position(|t| t.uid == tab_uid) {
                return Some((p, i));
            }
        }
        None
    }

    fn try_select_pending(&mut self, pane: usize, tab_index: usize) {
        let Some((uid, names)) = self.pending_select.clone() else {
            return;
        };
        let tab = &self.panes[pane].tabs[tab_index];
        if tab.uid != uid {
            return;
        }
        let mut selected = HashSet::new();
        for (i, e) in tab.snapshot.entries.iter().enumerate() {
            if names.iter().any(|n| n == &e.name) {
                selected.insert(i as u32);
            }
        }
        if selected.is_empty() {
            if !tab.loading {
                self.pending_select = None;
            }
            return;
        }
        let cursor = tab.view.iter().position(|&x| selected.contains(&x));
        let tab = &mut self.panes[pane].tabs[tab_index];
        tab.selection = selected;
        tab.cursor = cursor;
        self.pending_select = None;
        self.force_scroll_tab = Some(uid);
        self.active_pane = pane;
        self.panes[pane].active_tab = tab_index;
    }

    fn best_tab_uid_for_path(&self, dest: &Path) -> Option<u64> {
        let pane = self.active_pane.min(self.panes.len().saturating_sub(1));
        if let Some(p) = self.panes.get(pane) {
            if p.tab().path == dest {
                return Some(p.tab().uid);
            }
            if let Some(t) = p.tabs.iter().find(|t| t.path == dest) {
                return Some(t.uid);
            }
        }
        for (i, p) in self.panes.iter().enumerate() {
            if i == pane {
                continue;
            }
            if p.tab().path == dest {
                return Some(p.tab().uid);
            }
            if let Some(t) = p.tabs.iter().find(|t| t.path == dest) {
                return Some(t.uid);
            }
        }
        None
    }

    fn focus_tab_uid(&mut self, uid: u64) {
        if let Some((pane, ti)) = self.find_tab_by_uid(uid) {
            self.active_pane = pane;
            self.panes[pane].active_tab = ti;
            self.force_scroll_tab = Some(uid);
        }
    }

    /// After a copy/move/create, select the new items in the destination tab.
    fn select_created_items(&mut self, created: Vec<PathBuf>) {
        if created.is_empty() {
            return;
        }
        let Some(dest) = created[0].parent().map(Path::to_path_buf) else {
            return;
        };
        let names: Vec<String> = created
            .iter()
            .filter_map(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
            .collect();
        if names.is_empty() {
            return;
        }
        let Some(uid) = self.best_tab_uid_for_path(&dest) else {
            return;
        };
        self.pending_select = Some((uid, names));
        self.focus_tab_uid(uid);
    }

    fn handle_msg(&mut self, ctx: &egui::Context, msg: UiMsg) {
        match msg {
            UiMsg::Batch { token, entries, done, error } => {
                let Some((pane, ti)) = self.find_tab(token.pane, token.tab_uid) else {
                    return;
                };
                let tab = &mut self.panes[pane].tabs[ti];
                if token.generation != tab.generation {
                    return; // stale
                }
                tab.pending.extend(entries);
                let show_progressive = !done && tab.pending.len() <= 20_000;
                if done || show_progressive {
                    let entries = Arc::new(std::mem::take(&mut tab.pending));
                    if show_progressive {
                        // Keep accumulating: copy back for future batches.
                        tab.pending = (*entries).clone();
                    }
                    tab.snapshot = DirSnapshot {
                        path: tab.path.clone(),
                        entries,
                        generation: tab.generation,
                        complete: done,
                        error,
                        dir_count: 0,
                        file_count: 0,
                        file_bytes: 0,
                    };
                    tab.snapshot.recompute_counts();
                    tab.view = Arc::new((0..tab.snapshot.entries.len() as u32).collect());
                    if done {
                        tab.loading = false;
                        tab.pending = Vec::new();
                        self.rebuild_view(pane);
                    }
                }
            }
            UiMsg::View { token, view } => {
                let Some((pane, ti)) = self.find_tab(token.pane, token.tab_uid) else {
                    return;
                };
                {
                    let tab = &mut self.panes[pane].tabs[ti];
                    if token.generation != tab.generation {
                        return;
                    }
                    tab.view = Arc::new(view);
                    if let Some(names) = self.keep_selection.remove(&tab.uid) {
                        let name_set: HashSet<&str> = names.iter().map(|s| s.as_str()).collect();
                        tab.selection = tab
                            .snapshot
                            .entries
                            .iter()
                            .enumerate()
                            .filter(|(_, e)| name_set.contains(e.name.as_str()))
                            .map(|(i, _)| i as u32)
                            .collect();
                    }
                }
                self.try_select_pending(pane, ti);
            }
            UiMsg::Icon { key, image } => {
                self.icons.pending.remove(&key);
                let tex = image.map(|img| {
                    ctx.load_texture(
                        format!("icon-{key}"),
                        egui::ColorImage::from_rgba_unmultiplied(
                            [img.width as usize, img.height as usize],
                            &img.rgba,
                        ),
                        egui::TextureOptions::LINEAR,
                    )
                });
                self.icons.map.insert(key, tex);
            }
            UiMsg::DirSize { path, size } => {
                self.folder_size_pending.remove(&path);
                self.folder_sizes.insert(path, size);
            }
            UiMsg::SearchResults { query_id, results, done } => {
                if query_id == self.search.query_id {
                    self.search.results = results.clone();
                    self.search.running = !done;
                    self.search.selected = 0;
                }
                if query_id == self.palette.query_id {
                    self.palette.results =
                        results.into_iter().filter(|(_, is_dir)| *is_dir).collect();
                    self.palette.selected = 0;
                }
            }
            UiMsg::Preview { path, generation, content } => {
                if generation != self.preview.generation
                    || self.preview.path.as_deref() != Some(&path)
                {
                    return;
                }
                self.preview.loading = false;
                crate::preview::clear_content(&mut self.preview);
                self.preview.audio_ctl.stop();
                match content {
                    PreviewContent::Image { size, rgba } => {
                        self.preview.texture = Some(ctx.load_texture(
                            "preview",
                            egui::ColorImage::from_rgba_unmultiplied(size, &rgba),
                            egui::TextureOptions::LINEAR,
                        ));
                    }
                    PreviewContent::Text(t) => self.preview.text = Some(t),
                    PreviewContent::Info(i) => self.preview.info = Some(i),
                    PreviewContent::Hex { file_size, bytes } => {
                        self.preview.hex = Some(HexPreview { file_size, bytes });
                    }
                    PreviewContent::Audio { path, lines, duration_secs, cover } => {
                        if let Some((size, rgba)) = cover {
                            self.preview.texture = Some(ctx.load_texture(
                                "preview-cover",
                                egui::ColorImage::from_rgba_unmultiplied(size, &rgba),
                                egui::TextureOptions::LINEAR,
                            ));
                        }
                        let play_path = path.clone();
                        self.preview.audio = Some(AudioPreview {
                            path,
                            lines,
                            duration_secs,
                        });
                        self.preview.audio_ctl.play(play_path);
                    }
                    PreviewContent::Web { url, fallback_text } => {
                        self.preview.web_url = Some(url);
                        self.preview.web_fallback = fallback_text;
                    }
                }
            }
            UiMsg::ColumnValue { plugin, path, value } => {
                self.column_pending.remove(&(plugin, path.clone()));
                self.column_values.insert((plugin, path), value);
            }
            UiMsg::Checksum { path, value } => {
                self.checksum_pending.remove(&path);
                self.checksums.insert(path, value);
            }
            UiMsg::DirChanged { pane, tab_uid } => {
                if let Some((p, ti)) = self.find_tab(pane, tab_uid) {
                    if !self.panes[p].tabs[ti].loading {
                        self.request_listing_for(p, ti, true);
                    }
                }
            }
            UiMsg::DirsListed { path, dirs } => {
                self.tree_pending.remove(&path);
                self.tree_children.insert(path, dirs);
            }
            UiMsg::RecycleMeta { items } => {
                self.recycle_meta.clear();
                for item in items {
                    self.recycle_meta.insert(item.name.clone(), item);
                }
            }
            UiMsg::CompareResult { query_id, rows } => {
                if query_id == self.compare.query_id {
                    self.compare.rows = rows;
                    self.compare.running = false;
                    self.compare.selected.clear();
                }
            }
        }
    }

    fn handle_op_event(&mut self, ev: OpEvent) {
        match ev {
            OpEvent::Started { op_id, origin: _, label, total_bytes, total_files } => {
                self.ops_view.push(OpView {
                    op_id,
                    label,
                    done_bytes: 0,
                    total_bytes,
                    done_files: 0,
                    total_files,
                    current: String::new(),
                    finished: false,
                    error: None,
                    started: Instant::now(),
                });
                if self.ops_selected.is_none() {
                    self.ops_selected = Some(op_id);
                }
                if self.ops_view.len() > 12 {
                    self.ops_view.retain(|o| !o.finished || o.error.is_some());
                }
            }
            OpEvent::Progress { op_id, done_bytes, total_bytes, done_files, total_files, current } => {
                if let Some(v) = self.ops_view.iter_mut().find(|o| o.op_id == op_id) {
                    v.done_bytes = done_bytes;
                    v.total_bytes = total_bytes;
                    v.done_files = done_files;
                    v.total_files = total_files;
                    v.current = current;
                }
            }
            OpEvent::Conflict { op_id, source, dest } => {
                let prompt = ConflictPrompt { op_id, source, dest, apply_to_all: false };
                match self.settings.conflict_default {
                    ConflictDefault::Ask => {
                        if self.conflict.is_some() {
                            self.conflict_queue.push_back(prompt);
                        } else {
                            self.conflict = Some(prompt);
                        }
                    }
                    ConflictDefault::Overwrite => {
                        self.ops.resolve_conflict(op_id, ConflictResolution::Overwrite, true);
                    }
                    ConflictDefault::KeepBoth => {
                        self.ops.resolve_conflict(op_id, ConflictResolution::AutoRename, true);
                    }
                    ConflictDefault::Skip => {
                        self.ops.resolve_conflict(op_id, ConflictResolution::Skip, true);
                    }
                }
            }
            OpEvent::Done { op_id, origin, undo, refresh, created } => {
                if let Some(v) = self.ops_view.iter_mut().find(|o| o.op_id == op_id) {
                    v.finished = true;
                }
                let original = self.pending_ops.remove(&op_id);
                if origin == OpOrigin::User {
                    if let Some(u) = undo {
                        self.undo.record(u, original.into_iter().collect());
                    }
                }
                self.select_created_items(created);
                self.refresh_all_matching(&refresh);
            }
            OpEvent::Failed { op_id, origin: _, error } => {
                self.pending_ops.remove(&op_id);
                if let Some(v) = self.ops_view.iter_mut().find(|o| o.op_id == op_id) {
                    v.finished = true;
                    v.error = Some(error.clone());
                }
                self.toast(format!("Operation failed: {error}"), true);
            }
            OpEvent::Cancelled { op_id, origin: _ } => {
                self.pending_ops.remove(&op_id);
                if let Some(v) = self.ops_view.iter_mut().find(|o| o.op_id == op_id) {
                    v.finished = true;
                    v.error = Some("cancelled".into());
                }
            }
        }
    }

    pub fn toast(&mut self, msg: String, is_error: bool) {
        self.toasts.push((msg, Instant::now(), is_error));
    }

    // ----- commands -------------------------------------------------------

    /// Open an entry: navigate for dirs, shell-open for files (zip-aware).
    pub fn open_entry(&mut self, pane: usize, entry_index: u32) {
        let tab = self.panes[pane].tab();
        if sc_shell::recycle::is_recycle_path(&tab.path) {
            let Some(entry) = tab.snapshot.entries.get(entry_index as usize) else {
                return;
            };
            let name = entry.name.clone();
            self.restore_recycle_names(&[name]);
            return;
        }
        let Some(entry) = tab.snapshot.entries.get(entry_index as usize) else {
            return;
        };
        let full = tab.path.join(entry.name.replace('/', "\\"));
        let is_dir = entry.is_dir();
        let is_zip = !is_dir
            && full
                .extension()
                .map(|e| e.eq_ignore_ascii_case("zip"))
                .unwrap_or(false);
        if is_dir || is_zip {
            self.navigate(pane, full);
            return;
        }
        // Inside a zip? Extract to temp first.
        if let Some((zip_path, inner_dir)) = crate::vfs::split_zip_path(&tab.path) {
            let inner = if inner_dir.is_empty() {
                entry.name.clone()
            } else {
                format!("{}/{}", inner_dir, entry.name)
            };
            match crate::vfs::extract_to_temp(&zip_path, &inner) {
                Ok(tmp) => {
                    if let Err(e) = sc_shell::context::shell_open(&tmp) {
                        self.toast(e, true);
                    }
                }
                Err(e) => self.toast(e, true),
            }
            return;
        }
        if let Err(e) = sc_shell::context::shell_open(&full) {
            self.toast(e, true);
        }
    }

    pub fn other_pane(&self, pane: usize) -> usize {
        if self.panes.len() < 2 {
            0
        } else {
            1 - pane.min(1)
        }
    }

    pub fn copy_selection_to_clipboard(&mut self, pane: usize, cut: bool) {
        let paths = self.panes[pane].tab().paths_for_clipboard();
        if paths.is_empty() {
            return;
        }
        match sc_shell::clipboard::set_clipboard_files(&paths, cut) {
            Ok(()) => self.toast(
                format!("{} {} item(s)", if cut { "Cut" } else { "Copied" }, paths.len()),
                false,
            ),
            Err(e) => self.toast(e, true),
        }
    }

    pub fn copy_paths_to_clipboard(&mut self, pane: usize) {
        let paths = self.panes[pane].tab().paths_for_clipboard();
        if paths.is_empty() {
            return;
        }
        let text = paths
            .iter()
            .map(|p| p.display().to_string())
            .collect::<Vec<_>>()
            .join("\r\n");
        match sc_shell::clipboard::set_clipboard_text(&text) {
            Ok(()) => self.toast(
                format!("Copied {} path(s)", paths.len()),
                false,
            ),
            Err(e) => self.toast(e, true),
        }
    }

    pub fn paste_into(&mut self, pane: usize) {
        if sc_shell::recycle::is_recycle_path(&self.panes[pane].tab().path) {
            self.toast("Can't paste into the Recycle Bin".into(), true);
            return;
        }
        let Some((paths, is_cut)) = sc_shell::clipboard::get_clipboard_files() else {
            return;
        };
        if paths.is_empty() {
            return;
        }
        let dest = self.panes[pane].tab().path.clone();
        self.drop_files_into(paths, dest, is_cut);
    }

    /// Copy or move `sources` into `dest`. Empty after filtering (same folder,
    /// dropping a folder into itself) is a no-op.
    pub fn drop_files_into(&mut self, sources: Vec<PathBuf>, dest: PathBuf, is_move: bool) {
        if crate::vfs::split_zip_path(&dest).is_some() {
            self.toast("Cannot drop into a zip archive".into(), true);
            return;
        }
        if sc_shell::recycle::is_recycle_path(&dest) {
            self.toast("Cannot drop into the Recycle Bin".into(), true);
            return;
        }
        let sources: Vec<PathBuf> = sources
            .into_iter()
            .filter(|p| p.parent() != Some(dest.as_path()))
            .filter(|p| *p != dest && !dest.starts_with(p))
            .collect();
        if sources.is_empty() {
            return;
        }
        let op = if is_move {
            Operation::Move { sources, dest_dir: dest }
        } else {
            Operation::Copy { sources, dest_dir: dest }
        };
        self.submit_op(op);
    }

    /// Copy or move selection to the other pane (F5/F6 commander-style).
    pub fn transfer_to_other_pane(&mut self, pane: usize, is_move: bool) {
        let sources = self.panes[pane].tab().selected_paths();
        if sources.is_empty() {
            return;
        }
        let other = self.other_pane(pane);
        let dest = self.panes[other].tab().path.clone();
        // Zip source: extract instead of raw copy.
        if let Some((zip_path, inner_dir)) = crate::vfs::split_zip_path(&self.panes[pane].tab().path)
        {
            let tab = self.panes[pane].tab();
            let selected: Vec<(String, bool)> = tab
                .selection
                .iter()
                .filter_map(|&i| tab.snapshot.entries.get(i as usize))
                .map(|e| (e.name.clone(), e.is_dir()))
                .collect();
            let names: Vec<String> = selected.iter().map(|(n, _)| n.clone()).collect();
            let mut err = None;
            for (name, is_dir) in selected {
                let inner = if inner_dir.is_empty() {
                    name.clone()
                } else {
                    format!("{}/{}", inner_dir, name)
                };
                if let Err(e) = crate::vfs::extract_selection(&zip_path, &inner, is_dir, &dest) {
                    err = Some(e);
                }
            }
            match err {
                Some(e) => self.toast(e, true),
                None => self.toast("Extracted to other pane".into(), false),
            }
            let uid = self.panes[other].tab().uid;
            self.pending_select = Some((uid, names));
            self.focus_tab_uid(uid);
            self.refresh_all_matching(&[dest]);
            return;
        }
        let op = if is_move {
            Operation::Move { sources, dest_dir: dest }
        } else {
            Operation::Copy { sources, dest_dir: dest }
        };
        self.submit_op(op);
    }

    pub fn delete_selection(&mut self, pane: usize, shift_permanent: bool) {
        let tab = self.panes[pane].tab();
        if sc_shell::recycle::is_recycle_path(&tab.path) {
            let names = tab.selected_names();
            if names.is_empty() {
                return;
            }
            self.delete_recycle_names(&names);
            return;
        }
        let paths = tab.selected_paths();
        if paths.is_empty() {
            return;
        }
        let permanent = shift_permanent || self.settings.delete_permanent_default;
        let confirm = if permanent {
            self.settings.confirm_permanent_delete
        } else {
            self.settings.confirm_recycle_delete
        };
        if confirm {
            self.pending_delete = Some((paths, permanent));
        } else {
            self.submit_op(Operation::Delete { paths, recycle: !permanent });
        }
    }

    pub fn start_rename(&mut self, pane: usize) {
        if sc_shell::recycle::is_recycle_path(&self.panes[pane].tab().path) {
            return;
        }
        let tab = self.panes[pane].tab();
        let Some(&idx) = tab.selection.iter().next() else { return };
        let Some(entry) = tab.snapshot.entries.get(idx as usize) else { return };
        self.rename = Some(RenameState {
            pane,
            tab_uid: tab.uid,
            entry_index: idx,
            buffer: entry.name.clone(),
            focus_requested: true,
        });
    }

    pub fn commit_rename(&mut self) {
        let Some(r) = self.rename.take() else { return };
        let Some((pane, ti)) = self.find_tab(r.pane, r.tab_uid) else { return };
        let tab = &self.panes[pane].tabs[ti];
        let Some(entry) = tab.snapshot.entries.get(r.entry_index as usize) else {
            return;
        };
        let new_name = r.buffer.trim();
        if new_name.is_empty() || new_name == entry.name {
            return;
        }
        let from = tab.path.join(&entry.name);
        let to = tab.path.join(new_name);
        self.submit_op(Operation::Rename { from, to });
    }

    pub fn request_preview(&mut self, path: Option<PathBuf>) {
        if self.preview.path == path {
            return;
        }
        self.preview.generation += 1;
        self.preview.path = path.clone();
        crate::preview::clear_content(&mut self.preview);
        self.preview.audio_ctl.stop();
        crate::preview::destroy_web(&mut self.preview);
        if let Some(p) = path {
            if p.is_file() {
                self.preview.loading = true;
                self.engine.submit(Job::Preview { path: p, generation: self.preview.generation });
            } else {
                self.preview.loading = false;
            }
        } else {
            self.preview.loading = false;
        }
    }

    /// Close every floating window/dialog, leaving the main UI.
    pub fn dismiss_floating_ui(&mut self) -> bool {
        let mut dismissed = false;
        if self.search.open {
            self.search.open = false;
            dismissed = true;
        }
        if self.palette.open {
            self.palette.open = false;
            dismissed = true;
        }
        if self.preview.enabled {
            crate::preview::close(&mut self.preview);
            dismissed = true;
        }
        if self.show_settings {
            self.show_settings = false;
            dismissed = true;
        }
        if self.show_columns {
            self.show_columns = false;
            dismissed = true;
        }
        if self.show_about {
            self.show_about = false;
            dismissed = true;
        }
        if self.show_plugin_manager {
            self.show_plugin_manager = false;
            dismissed = true;
        }
        if self.show_everything_prompt {
            self.show_everything_prompt = false;
            dismissed = true;
        }
        if self.new_item.take().is_some() {
            dismissed = true;
        }
        if self.tag_edit.take().is_some() {
            dismissed = true;
        }
        if self.batch_rename.open {
            self.batch_rename.open = false;
            dismissed = true;
        }
        if self.pending_delete.take().is_some() {
            dismissed = true;
        }
        if let Some(c) = self.conflict.take() {
            self.ops.resolve_conflict(c.op_id, ConflictResolution::Cancel, false);
            while let Some(c) = self.conflict_queue.pop_front() {
                self.ops.resolve_conflict(c.op_id, ConflictResolution::Cancel, false);
            }
            dismissed = true;
        }
        if self.compare.open {
            self.compare.open = false;
            dismissed = true;
        }
        if self.row_ctx_menu.take().is_some() {
            dismissed = true;
        }
        if self.pane_bg_menu.take().is_some() {
            dismissed = true;
        }
        dismissed
    }

    pub fn open_search(&mut self) {
        let already_open = self.search.open;
        self.search.open = true;
        self.search.focus_requested = true;
        if !already_open {
            self.search.mode = SearchMode::NameHere;
            if !self.search.query.trim().is_empty() {
                self.run_search();
            }
        }
    }

    pub fn run_search(&mut self) {
        let query = self.search.query.trim().to_string();
        if query.is_empty() {
            self.search.results.clear();
            self.search.running = false;
            return;
        }
        let id = self.engine.new_search();
        self.search.query_id = id;
        self.search.running = true;
        let max = self.settings.search_max_results.max(1);
        let max_bytes = self.settings.content_search_max_mb.saturating_mul(1024 * 1024);
        match self.search.mode {
            SearchMode::NameGlobal => {
                self.engine.submit(Job::SearchNames {
                    query_id: id,
                    query,
                    max,
                    scope: None,
                    dirs_only: false,
                    near: Some(self.active_tab().path.clone()),
                });
            }
            SearchMode::NameHere => {
                let scope = self.active_tab().path.clone();
                self.engine.submit(Job::SearchNames {
                    query_id: id,
                    query,
                    max,
                    scope: Some(scope.clone()),
                    dirs_only: false,
                    near: Some(scope),
                });
            }
            SearchMode::Content => {
                let scope = self.active_tab().path.clone();
                self.engine.submit(Job::ContentSearch {
                    query_id: id,
                    scope,
                    needle: query,
                    max_results: max,
                    max_file_size: max_bytes,
                });
            }
        }
    }

    pub fn run_palette(&mut self) {
        let query = self.palette.query.trim().to_string();
        if query.is_empty() {
            self.palette.results.clear();
            return;
        }
        let id = self.engine.new_search();
        self.palette.query_id = id;
        let max = self.settings.search_max_results.max(1);
        self.engine.submit(Job::SearchNames {
            query_id: id,
            query,
            max,
            scope: None,
            dirs_only: true,
            near: Some(self.active_tab().path.clone()),
        });
    }

    /// Open a search hit: files in the shell, folders in a new tab.
    pub fn activate_search_hit(&mut self, path: PathBuf, is_dir: bool) {
        if is_dir {
            let pane = self.active_pane;
            self.open_folder_in_new_tab(pane, path);
        } else {
            let _ = sc_shell::context::shell_open(&path);
        }
    }

    pub fn restore_recycle_names(&mut self, names: &[String]) {
        let mut parsing = Vec::new();
        let mut refresh = Vec::new();
        for n in names {
            if let Some(item) = self.recycle_meta.get(n) {
                parsing.push(item.parsing_name.clone());
                if let Some(orig) = &item.original_path {
                    if let Some(parent) = orig.parent() {
                        refresh.push(parent.to_path_buf());
                    }
                }
            }
        }
        if parsing.is_empty() {
            self.toast("Nothing to restore".into(), true);
            return;
        }
        self.submit_op(Operation::RecycleRestore {
            parsing_names: parsing,
            refresh,
        });
    }

    pub fn delete_recycle_names(&mut self, names: &[String]) {
        let parsing: Vec<String> = names
            .iter()
            .filter_map(|n| self.recycle_meta.get(n).map(|i| i.parsing_name.clone()))
            .collect();
        if parsing.is_empty() {
            return;
        }
        self.submit_op(Operation::RecycleDelete {
            parsing_names: parsing,
        });
    }

    pub fn answer_conflict(&mut self, res: ConflictResolution, apply_to_all: bool) {
        if let Some(c) = self.conflict.take() {
            self.ops.resolve_conflict(c.op_id, res, apply_to_all);
        }
        self.conflict = self.conflict_queue.pop_front();
    }

    // ----- session ---------------------------------------------------------

    pub fn session_snapshot(&self) -> Session {
        Session {
            layout: self.layout,
            active_pane: self.active_pane,
            panes: self
                .panes
                .iter()
                .map(|p| SessionPane {
                    tabs: p
                        .tabs
                        .iter()
                        .map(|t| SessionTab {
                            path: t.path.clone(),
                            locked: t.locked,
                            sort: t.sort,
                            custom_title: t.custom_title.clone(),
                            color: t.color.clone(),
                        })
                        .collect(),
                    active_tab: p.active_tab,
                })
                .collect(),
            show_hidden: self.show_hidden,
            theme: self.theme.name.to_string(),
            favorites: self.settings.session.favorites.clone(),
            window: None,
            split_ratio: self.split_ratio,
            sidebar_width: self.sidebar_width,
            preview_width: self.preview_width,
            preview_height: self.preview_height,
            unc_roots: self.settings.session.unc_roots.clone(),
            sidebar_wsl_open: self.settings.session.sidebar_wsl_open,
            sidebar_network_open: self.settings.session.sidebar_network_open,
            sidebar_favorites_open: self.settings.session.sidebar_favorites_open,
            sidebar_drives_open: self.settings.session.sidebar_drives_open,
            sidebar_user_folders_open: self.settings.session.sidebar_user_folders_open,
            sidebar_tree_open: self.settings.session.sidebar_tree_open,
            tree_expanded: self.tree_open.iter().cloned().collect(),
        }
    }

    pub fn save_session(&mut self) {
        self.settings.session = self.session_snapshot();
        config::save_settings(&self.settings);
        self.last_session_save = Instant::now();
    }

    pub fn persist_settings(&mut self) {
        sc_shell::everything::set_preferred_exe(
            (!self.settings.everything_exe.trim().is_empty())
                .then(|| PathBuf::from(self.settings.everything_exe.trim())),
        );
        config::save_settings(&self.settings);
    }

    /// Re-apply theme, accent override, and UI scale to the egui context.
    pub fn apply_appearance(&mut self, ctx: &egui::Context) {
        self.theme = theme::by_name(&self.settings.session.theme);
        theme::apply_accent_override(&mut self.theme, &self.settings.accent);
        theme::apply(ctx, &self.theme);
        ctx.set_zoom_factor(self.settings.ui_scale.clamp(0.75, 2.0));
    }

    pub fn toggle_layout(&mut self) {
        self.layout = match self.layout {
            PaneLayout::Single => PaneLayout::Dual(SplitDirection::Vertical),
            PaneLayout::Dual(SplitDirection::Vertical) => {
                PaneLayout::Dual(SplitDirection::Horizontal)
            }
            PaneLayout::Dual(SplitDirection::Horizontal) => PaneLayout::Single,
        };
    }

    pub fn set_theme(&mut self, name: &str) {
        self.settings.session.theme = name.to_string();
        self.theme = theme::by_name(name);
        theme::apply_accent_override(&mut self.theme, &self.settings.accent);
    }

    pub fn set_accent(&mut self, hex: &str) {
        self.settings.accent = hex.trim().trim_start_matches('#').to_string();
        if theme::is_amoled(&self.settings.session.theme) {
            self.settings.session.theme = "amoled".into();
        }
        self.theme = theme::by_name(&self.settings.session.theme);
        theme::apply_accent_override(&mut self.theme, &self.settings.accent);
    }

    pub fn sort_by(&mut self, pane: usize, key: SortKey) {
        let tab = self.panes[pane].tab_mut();
        if tab.sort.key == key {
            tab.sort.ascending = !tab.sort.ascending;
        } else {
            tab.sort = SortSpec { key, ascending: true, dirs_first: tab.sort.dirs_first };
        }
        self.rebuild_view(pane);
    }

    /// Queue a folder-size job, capped so huge listings cannot flood the pool.
    pub fn request_dir_size(&mut self, path: PathBuf) {
        if self.folder_sizes.contains_key(&path) || self.folder_size_pending.contains(&path) {
            return;
        }
        if self.folder_size_pending.len() >= 8 {
            return;
        }
        self.folder_size_pending.insert(path.clone());
        self.engine.submit(Job::DirSize { path });
    }

    /// Move or reorder a tab. `to_index` is the insertion index in the destination pane.
    pub fn relocate_tab(&mut self, from_pane: usize, from_index: usize, to_pane: usize, to_index: usize) {
        if from_pane >= self.panes.len() || to_pane >= self.panes.len() {
            return;
        }
        if from_pane == to_pane {
            self.panes[from_pane].reorder_tab(from_index, to_index);
            self.active_pane = from_pane;
            return;
        }
        let Some(tab) = self.panes[from_pane].take_tab(from_index) else {
            return;
        };
        self.panes[to_pane].insert_tab(to_index, tab);
        self.active_pane = to_pane;
    }

    pub fn find_tab_by_uid(&self, uid: u64) -> Option<(usize, usize)> {
        for (p, pane) in self.panes.iter().enumerate() {
            for (i, t) in pane.tabs.iter().enumerate() {
                if t.uid == uid {
                    return Some((p, i));
                }
            }
        }
        None
    }
}

pub(crate) fn split_cmdline(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_quotes = false;
    for c in s.chars() {
        match c {
            '"' => in_quotes = !in_quotes,
            c if c.is_whitespace() && !in_quotes => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            c => cur.push(c),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// Replace the executable token in a command line, keeping remaining arguments.
pub(crate) fn replace_cmdline_program(cmd: &str, exe: &str) -> String {
    let quoted = if exe.chars().any(char::is_whitespace) {
        format!("\"{exe}\"")
    } else {
        exe.to_string()
    };
    let parts = split_cmdline(cmd);
    if parts.len() <= 1 {
        if cmd.contains("{path}") {
            quoted
        } else {
            format!("{quoted} \"{{path}}\"")
        }
    } else {
        let rest: Vec<String> = parts
            .iter()
            .skip(1)
            .map(|p| {
                if p.chars().any(char::is_whitespace) {
                    format!("\"{p}\"")
                } else {
                    p.clone()
                }
            })
            .collect();
        format!("{quoted} {}", rest.join(" "))
    }
}

/// Windows file/folder name checks used by the New file/folder prompt.
pub(crate) fn invalid_item_name(name: &str) -> Option<&'static str> {
    let name = name.trim();
    if name.is_empty() {
        return Some("Enter a name");
    }
    if name == "." || name == ".." {
        return Some("Invalid name");
    }
    if name.chars().any(|c| matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*')) {
        return Some("Name contains invalid characters: <>:\"/\\|?*");
    }
    if name.ends_with('.') || name.ends_with(' ') {
        return Some("Name can't end with a period or space");
    }
    let stem = name.split('.').next().unwrap_or(name);
    const RESERVED: &[&str] = &[
        "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7",
        "COM8", "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
    ];
    if RESERVED.iter().any(|r| stem.eq_ignore_ascii_case(r)) {
        return Some("That name is reserved by Windows");
    }
    None
}
