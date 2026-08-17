use crate::snapshot::DirSnapshot;
use crate::sort::SortSpec;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum SplitDirection {
    Vertical,   // panes side by side
    Horizontal, // panes stacked
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum PaneLayout {
    Single,
    Dual(SplitDirection),
}

static TAB_UID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

/// One browsing tab. Owns its snapshot, view, history, and selection.
pub struct TabState {
    /// Stable identity used to route async results to this tab.
    pub uid: u64,
    pub path: PathBuf,
    /// Entries accumulated while a listing is streaming in.
    pub pending: Vec<crate::entry::FsEntry>,
    /// True while a listing is being loaded.
    pub loading: bool,
    pub snapshot: DirSnapshot,
    /// Sorted + filtered indices into `snapshot.entries`.
    pub view: Arc<Vec<u32>>,
    pub sort: SortSpec,
    pub filter: String,
    pub locked: bool,
    /// Selected entry indices (into `snapshot.entries`, not the view).
    pub selection: HashSet<u32>,
    /// Anchor for shift-selection, as a view position.
    pub cursor: Option<usize>,
    pub history_back: Vec<PathBuf>,
    pub history_fwd: Vec<PathBuf>,
    pub generation: u64,
    /// Show all files under this folder recursively (flatten branch view).
    pub flatten: bool,
    /// Optional user-assigned tab name. `None` uses the folder name.
    pub custom_title: Option<String>,
    /// Optional tab color as RGB hex (e.g. "e84a4a"). `None` = default.
    pub color: Option<String>,
}

impl TabState {
    pub fn new(path: PathBuf) -> Self {
        Self {
            uid: TAB_UID.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            snapshot: DirSnapshot::empty(path.clone(), 0),
            path,
            pending: Vec::new(),
            loading: false,
            view: Arc::new(Vec::new()),
            sort: SortSpec::default(),
            filter: String::new(),
            locked: false,
            selection: HashSet::new(),
            cursor: None,
            history_back: Vec::new(),
            history_fwd: Vec::new(),
            generation: 0,
            flatten: false,
            custom_title: None,
            color: None,
        }
    }

    pub fn folder_name(&self) -> String {
        self.path
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.path.to_string_lossy().into_owned())
    }

    pub fn title(&self) -> String {
        let name = self
            .custom_title
            .as_ref()
            .filter(|s| !s.is_empty())
            .cloned()
            .unwrap_or_else(|| self.folder_name());
        if self.locked {
            format!("\u{1F512} {name}")
        } else {
            name
        }
    }

    /// Record navigation to a new path (pushes current path to back history).
    pub fn navigate(&mut self, to: PathBuf) {
        if to == self.path {
            return;
        }
        self.history_back.push(std::mem::replace(&mut self.path, to));
        self.history_fwd.clear();
        self.on_path_changed();
    }

    pub fn go_back(&mut self) -> bool {
        if let Some(prev) = self.history_back.pop() {
            self.history_fwd.push(std::mem::replace(&mut self.path, prev));
            self.on_path_changed();
            true
        } else {
            false
        }
    }

    pub fn go_forward(&mut self) -> bool {
        if let Some(next) = self.history_fwd.pop() {
            self.history_back.push(std::mem::replace(&mut self.path, next));
            self.on_path_changed();
            true
        } else {
            false
        }
    }

    pub fn go_up(&mut self) -> bool {
        if let Some(parent) = self.path.parent().map(Path::to_path_buf) {
            self.navigate(parent);
            true
        } else {
            false
        }
    }

    fn on_path_changed(&mut self) {
        self.selection.clear();
        self.cursor = None;
        self.filter.clear();
        self.flatten = false;
        self.generation += 1;
    }

    /// Names of currently selected entries.
    pub fn selected_names(&self) -> Vec<String> {
        self.selection
            .iter()
            .filter_map(|&i| self.snapshot.entries.get(i as usize))
            .map(|e| e.name.clone())
            .collect()
    }

    pub fn selected_paths(&self) -> Vec<PathBuf> {
        self.selected_names().iter().map(|n| self.path.join(n)).collect()
    }

    pub fn cursor_path(&self) -> Option<PathBuf> {
        let pos = self.cursor?;
        let ei = *self.view.get(pos)?;
        let name = &self.snapshot.entries.get(ei as usize)?.name;
        Some(self.path.join(name))
    }

    /// Selected items, or the focused/cursor row when nothing is selected.
    pub fn paths_for_clipboard(&self) -> Vec<PathBuf> {
        let paths = self.selected_paths();
        if !paths.is_empty() {
            return paths;
        }
        self.cursor_path().into_iter().collect()
    }
}

/// One pane holds a set of tabs.
pub struct PaneState {
    pub tabs: Vec<TabState>,
    pub active_tab: usize,
}

impl PaneState {
    pub fn new(path: PathBuf) -> Self {
        Self { tabs: vec![TabState::new(path)], active_tab: 0 }
    }

    pub fn tab(&self) -> &TabState {
        &self.tabs[self.active_tab.min(self.tabs.len() - 1)]
    }

    pub fn tab_mut(&mut self) -> &mut TabState {
        let i = self.active_tab.min(self.tabs.len() - 1);
        &mut self.tabs[i]
    }

    pub fn add_tab(&mut self, path: PathBuf) {
        self.tabs.push(TabState::new(path));
        self.active_tab = self.tabs.len() - 1;
    }

    /// Insert a tab immediately after the active one without switching to it.
    /// Returns the new tab's index.
    pub fn insert_tab_beside(&mut self, path: PathBuf) -> usize {
        let index = (self.active_tab + 1).min(self.tabs.len());
        self.tabs.insert(index, TabState::new(path));
        index
    }

    /// Close a tab; keeps at least one tab alive. Returns false if refused.
    pub fn close_tab(&mut self, index: usize) -> bool {
        if self.tabs.len() <= 1 || index >= self.tabs.len() || self.tabs[index].locked {
            return false;
        }
        self.tabs.remove(index);
        if self.active_tab >= self.tabs.len() {
            self.active_tab = self.tabs.len() - 1;
        }
        true
    }

    /// Remove a tab so it can be moved to another pane. Refuses the last tab.
    pub fn take_tab(&mut self, index: usize) -> Option<TabState> {
        if self.tabs.len() <= 1 || index >= self.tabs.len() {
            return None;
        }
        let tab = self.tabs.remove(index);
        if self.active_tab >= self.tabs.len() {
            self.active_tab = self.tabs.len() - 1;
        } else if self.active_tab > index {
            self.active_tab -= 1;
        }
        Some(tab)
    }

    pub fn insert_tab(&mut self, index: usize, tab: TabState) {
        let index = index.min(self.tabs.len());
        self.tabs.insert(index, tab);
        self.active_tab = index;
    }

    /// Move a tab within this pane. `to` is the insertion index before removal.
    pub fn reorder_tab(&mut self, from: usize, mut to: usize) {
        if from >= self.tabs.len() || from == to {
            return;
        }
        to = to.min(self.tabs.len());
        let tab = self.tabs.remove(from);
        if from < to {
            to -= 1;
        }
        to = to.min(self.tabs.len());
        self.tabs.insert(to, tab);
        self.active_tab = to;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pane_with(n: usize) -> PaneState {
        let mut p = PaneState::new(PathBuf::from("C:\\a"));
        for i in 1..n {
            p.add_tab(PathBuf::from(format!("C:\\t{i}")));
        }
        p
    }

    #[test]
    fn title_uses_custom_then_folder() {
        let mut t = TabState::new(PathBuf::from("C:\\Users\\docs"));
        assert_eq!(t.folder_name(), "docs");
        assert_eq!(t.title(), "docs");
        t.custom_title = Some("Work".into());
        assert_eq!(t.title(), "Work");
        t.locked = true;
        assert!(t.title().contains("Work"));
    }

    #[test]
    fn insert_tab_beside_keeps_focus() {
        let mut p = pane_with(3);
        p.active_tab = 0;
        let idx = p.insert_tab_beside(PathBuf::from("C:\\new"));
        assert_eq!(idx, 1);
        assert_eq!(p.active_tab, 0);
        assert_eq!(p.tabs[1].path, PathBuf::from("C:\\new"));
        assert_eq!(p.tabs.len(), 4);

        p.active_tab = 3;
        let idx = p.insert_tab_beside(PathBuf::from("C:\\end"));
        assert_eq!(idx, 4);
        assert_eq!(p.active_tab, 3);
        assert_eq!(p.tabs[4].path, PathBuf::from("C:\\end"));
    }

    #[test]
    fn take_refuses_last_tab() {
        let mut p = pane_with(1);
        assert!(p.take_tab(0).is_none());
        assert_eq!(p.tabs.len(), 1);
    }

    #[test]
    fn take_and_insert_moves_tab() {
        let mut a = pane_with(2);
        let mut b = pane_with(1);
        let uid = a.tabs[0].uid;
        let tab = a.take_tab(0).expect("can take when >1 tabs");
        assert_eq!(tab.uid, uid);
        assert_eq!(a.tabs.len(), 1);
        b.insert_tab(0, tab);
        assert_eq!(b.tabs.len(), 2);
        assert_eq!(b.active_tab, 0);
        assert_eq!(b.tabs[0].uid, uid);
    }

    #[test]
    fn reorder_within_pane() {
        let mut p = pane_with(3);
        let uids: Vec<u64> = p.tabs.iter().map(|t| t.uid).collect();
        p.reorder_tab(0, 2);
        assert_eq!(p.tabs[0].uid, uids[1]);
        assert_eq!(p.tabs[1].uid, uids[0]);
        assert_eq!(p.tabs[2].uid, uids[2]);
        assert_eq!(p.active_tab, 1);
    }
}

/// Serializable session (persisted across restarts).
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
pub struct Session {
    pub layout: PaneLayout,
    pub active_pane: usize,
    pub panes: Vec<SessionPane>,
    #[serde(default)]
    pub show_hidden: bool,
    #[serde(default)]
    pub theme: String,
    #[serde(default)]
    pub favorites: Vec<PathBuf>,
    #[serde(default)]
    pub window: Option<(f32, f32, f32, f32)>,
    /// Fraction of the split occupied by pane 0 (0.15..0.85).
    #[serde(default = "default_split_ratio")]
    pub split_ratio: f32,
    #[serde(default = "default_true")]
    pub sidebar_favorites_open: bool,
    #[serde(default = "default_true")]
    pub sidebar_drives_open: bool,
    #[serde(default = "default_true")]
    pub sidebar_user_folders_open: bool,
    #[serde(default = "default_true")]
    pub sidebar_tree_open: bool,
    /// Expanded folder-tree paths.
    #[serde(default)]
    pub tree_expanded: Vec<PathBuf>,
    /// Width of the navigation sidebar in points.
    #[serde(default = "default_sidebar_width")]
    pub sidebar_width: f32,
    /// Docked preview pane size.
    #[serde(default = "default_preview_width")]
    pub preview_width: f32,
    #[serde(default = "default_preview_height")]
    pub preview_height: f32,
    /// Remembered UNC roots shown in the Network sidebar.
    #[serde(default)]
    pub unc_roots: Vec<PathBuf>,
    #[serde(default = "default_true")]
    pub sidebar_wsl_open: bool,
    #[serde(default = "default_true")]
    pub sidebar_network_open: bool,
}

fn default_split_ratio() -> f32 {
    0.5
}

fn default_sidebar_width() -> f32 {
    210.0
}

fn default_preview_width() -> f32 {
    360.0
}

fn default_preview_height() -> f32 {
    240.0
}

fn default_true() -> bool {
    true
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
pub struct SessionPane {
    pub tabs: Vec<SessionTab>,
    pub active_tab: usize,
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
pub struct SessionTab {
    pub path: PathBuf,
    pub locked: bool,
    pub sort: SortSpec,
    #[serde(default)]
    pub custom_title: Option<String>,
    #[serde(default)]
    pub color: Option<String>,
}

impl Default for Session {
    fn default() -> Self {
        let home = dirs_home();
        Self {
            layout: PaneLayout::Dual(SplitDirection::Vertical),
            active_pane: 0,
            panes: vec![
                SessionPane {
                    tabs: vec![SessionTab { path: home.clone(), locked: false, sort: SortSpec::default(), custom_title: None, color: None }],
                    active_tab: 0,
                },
                SessionPane {
                    tabs: vec![SessionTab { path: home, locked: false, sort: SortSpec::default(), custom_title: None, color: None }],
                    active_tab: 0,
                },
            ],
            show_hidden: false,
            theme: "amoled".into(),
            favorites: Vec::new(),
            window: None,
            split_ratio: 0.5,
            sidebar_favorites_open: true,
            sidebar_drives_open: true,
            sidebar_user_folders_open: true,
            sidebar_tree_open: true,
            tree_expanded: Vec::new(),
            sidebar_width: 210.0,
            preview_width: 360.0,
            preview_height: 240.0,
            unc_roots: Vec::new(),
            sidebar_wsl_open: true,
            sidebar_network_open: true,
        }
    }
}

pub fn dirs_home() -> PathBuf {
    std::env::var_os("USERPROFILE")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("C:\\"))
}
