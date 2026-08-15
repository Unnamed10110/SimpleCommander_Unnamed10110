//! Plugin manifests and the persisted plugin registry.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Declared by the plugin itself (returned from its `sc_manifest` export).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PluginManifest {
    pub name: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub description: String,
    /// Any of: "command", "column", "preview", "vfs".
    #[serde(default)]
    pub kinds: Vec<String>,
    /// For column/preview plugins: file extensions handled (lowercase, no dot).
    #[serde(default)]
    pub extensions: Vec<String>,
    /// Requested capabilities: "read-files", "write-files".
    #[serde(default)]
    pub permissions: Vec<String>,
    /// Column header, for column plugins.
    #[serde(default)]
    pub column_title: String,
    /// Menu label, for command plugins.
    #[serde(default)]
    pub command_label: String,
}

/// User-side record: which plugins are installed/enabled/approved.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct PluginRegistry {
    #[serde(default)]
    pub plugins: Vec<PluginRecord>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PluginRecord {
    pub path: PathBuf,
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Permissions the user has granted.
    #[serde(default)]
    pub granted: Vec<String>,
}

fn default_true() -> bool {
    true
}

impl PluginRegistry {
    pub fn load(path: &Path) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|s| toml::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, path: &Path) {
        if let Ok(s) = toml::to_string_pretty(self) {
            let _ = std::fs::write(path, s);
        }
    }
}
