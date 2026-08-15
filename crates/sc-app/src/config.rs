//! Portable configuration: settings live next to the executable when that
//! directory is writable (portable mode, XYplorer-style), otherwise in
//! %APPDATA%\SimpleCommander. Writes are crash-safe (write + rename).

use crate::keymap::Keymap;
use sc_core::state::Session;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// A row-colorization rule (XYplorer "color filters").
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ColorRule {
    /// Wildcard pattern matched against the file name (e.g. "*.rs").
    pub pattern: String,
    /// RGB hex like "ff8800".
    pub color: String,
}

/// What to do when a copy/move target already exists.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ConflictDefault {
    #[default]
    Ask,
    Overwrite,
    KeepBoth,
    Skip,
}

/// Layout used when "restore session" is off, and the default for new windows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum DefaultLayout {
    #[default]
    DualVertical,
    DualHorizontal,
    Single,
}

impl DefaultLayout {
    pub fn to_pane_layout(self) -> sc_core::state::PaneLayout {
        use sc_core::state::{PaneLayout, SplitDirection};
        match self {
            Self::DualVertical => PaneLayout::Dual(SplitDirection::Vertical),
            Self::DualHorizontal => PaneLayout::Dual(SplitDirection::Horizontal),
            Self::Single => PaneLayout::Single,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::DualVertical => "Dual vertical",
            Self::DualHorizontal => "Dual horizontal",
            Self::Single => "Single pane",
        }
    }
}

/// One file-list column. `id` is `index`/`name`/`size`/`type`/`modified`/`created`/`sha256`
/// or `plugin:<name>`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ColumnPref {
    pub id: String,
    pub visible: bool,
}

pub fn default_columns() -> Vec<ColumnPref> {
    vec![
        ColumnPref { id: "index".into(), visible: true },
        ColumnPref { id: "name".into(), visible: true },
        ColumnPref { id: "size".into(), visible: true },
        ColumnPref { id: "type".into(), visible: true },
        ColumnPref { id: "modified".into(), visible: true },
        ColumnPref { id: "created".into(), visible: false },
        ColumnPref { id: "sha256".into(), visible: false },
    ]
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub session: Session,
    pub color_rules: Vec<ColorRule>,

    // ----- appearance -----
    /// Accent color override as RGB hex (e.g. "2fb8ff"); empty = theme default.
    pub accent: String,
    /// UI zoom factor (1.0 = 100%).
    pub ui_scale: f32,
    /// File list row height in points.
    pub row_height: f32,
    pub striped_rows: bool,
    pub show_icons: bool,

    // ----- behavior -----
    /// Open items with a single click instead of double click.
    pub single_click_open: bool,
    pub confirm_permanent_delete: bool,
    pub confirm_recycle_delete: bool,
    /// Type-ahead search reset timeout (milliseconds).
    pub type_ahead_ms: u64,
    /// Session autosave interval (seconds).
    pub autosave_secs: u64,
    /// Restore tabs/panes from the previous session on start.
    pub restore_session: bool,
    /// Remember per-tab sort order across restarts.
    pub remember_sort: bool,
    pub default_layout: DefaultLayout,

    // ----- file operations -----
    /// If true, plain Del deletes permanently instead of recycling.
    pub delete_permanent_default: bool,
    pub conflict_default: ConflictDefault,

    // ----- search & index -----
    /// Build the background filename index at startup.
    pub index_enabled: bool,
    pub search_max_results: usize,
    /// Content search skips files larger than this (MB).
    pub content_search_max_mb: u64,
    /// If true, do not offer to install Everything when it is missing.
    #[serde(default)]
    pub everything_prompt_dismissed: bool,

    /// File-list columns (order + visibility). Name is always shown.
    #[serde(default = "default_columns")]
    pub columns: Vec<ColumnPref>,

    /// Command used by the toolbar / Open terminal shortcut. `{path}` is replaced
    /// with the active folder.
    #[serde(default = "default_terminal_command")]
    pub terminal_command: String,

    /// Keyboard shortcuts. Missing keys in settings.toml keep the defaults.
    #[serde(default)]
    pub keymap: Keymap,
}

fn default_terminal_command() -> String {
    "wt.exe -d \"{path}\"".into()
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            session: Session::default(),
            color_rules: Vec::new(),
            accent: String::new(),
            ui_scale: 1.0,
            row_height: 22.0,
            striped_rows: true,
            show_icons: true,
            single_click_open: false,
            confirm_permanent_delete: true,
            confirm_recycle_delete: false,
            type_ahead_ms: 800,
            autosave_secs: 30,
            restore_session: true,
            remember_sort: true,
            default_layout: DefaultLayout::DualVertical,
            delete_permanent_default: false,
            conflict_default: ConflictDefault::Ask,
            index_enabled: true,
            search_max_results: 500,
            content_search_max_mb: 16,
            everything_prompt_dismissed: false,
            columns: default_columns(),
            terminal_command: default_terminal_command(),
            keymap: Keymap::default(),
        }
    }
}

pub fn config_dir() -> PathBuf {
    // Portable mode: settings next to the exe if writable.
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let probe = dir.join(".sc-write-probe");
            if std::fs::write(&probe, b"x").is_ok() {
                let _ = std::fs::remove_file(&probe);
                return dir.to_path_buf();
            }
        }
    }
    let base = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let dir = base.join("SimpleCommander");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

pub fn settings_path() -> PathBuf {
    config_dir().join("settings.toml")
}

pub fn tags_db_path() -> PathBuf {
    config_dir().join("sc-tags.db")
}

pub fn plugins_registry_path() -> PathBuf {
    config_dir().join("plugins.toml")
}

pub fn plugins_dir() -> PathBuf {
    let d = config_dir().join("plugins");
    let _ = std::fs::create_dir_all(&d);
    d
}

pub fn load_settings() -> Settings {
    let mut settings = std::fs::read_to_string(settings_path())
        .ok()
        .and_then(|s| toml::from_str(&s).ok())
        .unwrap_or_else(|| Settings {
            color_rules: vec![
                ColorRule { pattern: "*.zip".into(), color: "b08cff".into() },
                ColorRule { pattern: "*.exe".into(), color: "7fd48a".into() },
            ],
            ..Settings::default()
        });
    if settings.columns.is_empty() || !settings.columns.iter().any(|c| c.id == "name") {
        settings.columns = default_columns();
    } else if !settings.columns.iter().any(|c| c.id == "index") {
        settings.columns.insert(
            0,
            ColumnPref {
                id: "index".into(),
                visible: true,
            },
        );
    }
    if !settings.columns.iter().any(|c| c.id == "sha256") {
        settings.columns.push(ColumnPref {
            id: "sha256".into(),
            visible: false,
        });
    }
    settings
}

/// Crash-safe save: write to a temp file, then rename over the target.
pub fn save_settings(settings: &Settings) {
    let Ok(body) = toml::to_string_pretty(settings) else {
        return;
    };
    let path = settings_path();
    let tmp = path.with_extension("toml.tmp");
    if std::fs::write(&tmp, body).is_ok() {
        let _ = std::fs::rename(&tmp, &path);
    }
}
