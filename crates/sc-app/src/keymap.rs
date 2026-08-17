//! User-editable keyboard shortcuts. Stored in settings.toml as strings
//! like `"Ctrl+Shift+C"` or `"F7"`.

use egui::{Event, Key, Modifiers};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Chord {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub key: String,
}

impl Chord {
    pub fn new(ctrl: bool, alt: bool, shift: bool, key: &str) -> Self {
        Self { ctrl, alt, shift, key: key.into() }
    }

    pub fn unbound() -> Self {
        Self { ctrl: false, alt: false, shift: false, key: String::new() }
    }

    pub fn is_unbound(&self) -> bool {
        self.key.is_empty()
    }

    pub fn key(&self) -> Option<Key> {
        parse_key(&self.key)
    }

    pub fn label(&self) -> String {
        if self.is_unbound() {
            return "None".into();
        }
        let mut s = String::new();
        if self.ctrl {
            s.push_str("Ctrl+");
        }
        if self.alt {
            s.push_str("Alt+");
        }
        if self.shift {
            s.push_str("Shift+");
        }
        let key = self
            .key()
            .map(|k| {
                let sym = k.symbol_or_name();
                if sym.chars().count() == 1 {
                    sym.to_string()
                } else {
                    k.name().to_string()
                }
            })
            .unwrap_or_else(|| self.key.clone());
        s.push_str(&key);
        s
    }

    /// Stable `Ctrl+Shift+C` / `F7` form written to settings.toml.
    pub fn to_storage(&self) -> String {
        if self.is_unbound() {
            return String::new();
        }
        let mut s = String::new();
        if self.ctrl {
            s.push_str("Ctrl+");
        }
        if self.alt {
            s.push_str("Alt+");
        }
        if self.shift {
            s.push_str("Shift+");
        }
        s.push_str(&self.key);
        s
    }

    pub fn parse(s: &str) -> Option<Self> {
        if s.trim().is_empty() {
            return Some(Self::unbound());
        }
        let mut ctrl = false;
        let mut alt = false;
        let mut shift = false;
        let mut key = None;
        for part in s.split('+') {
            let p = part.trim();
            if p.is_empty() {
                continue;
            }
            match p.to_ascii_lowercase().as_str() {
                "ctrl" | "control" | "cmd" | "command" => ctrl = true,
                "alt" => alt = true,
                "shift" => shift = true,
                _ => {
                    let k = parse_key(p)?;
                    key = Some(format!("{k:?}"));
                }
            }
        }
        Some(Self { ctrl, alt, shift, key: key? })
    }

    pub fn from_egui(modifiers: Modifiers, key: Key) -> Self {
        Self {
            ctrl: modifiers.ctrl || modifiers.command,
            alt: modifiers.alt,
            shift: modifiers.shift,
            key: format!("{key:?}"),
        }
    }

    pub fn modifiers(&self) -> Modifiers {
        let mut m = Modifiers::NONE;
        if self.ctrl {
            m = m | Modifiers::CTRL;
        }
        if self.alt {
            m = m | Modifiers::ALT;
        }
        if self.shift {
            m = m | Modifiers::SHIFT;
        }
        m
    }

    pub fn consume(&self, ctx: &egui::Context) -> bool {
        let Some(k) = self.key() else { return false };
        ctx.input_mut(|i| i.consume_key(self.modifiers(), k))
    }

    /// True when the current modifier set matches this chord (key ignored).
    pub fn matches_modifiers(&self, m: Modifiers) -> bool {
        let ctrl_down = m.command || m.ctrl;
        ctrl_down == self.ctrl && m.alt == self.alt && m.shift == self.shift
    }
}

/// egui-winit turns Ctrl+C/X/V into these events and never emits a Key event.
pub fn take_copy_event(ctx: &egui::Context) -> bool {
    take_clipboard_event(ctx, |e| matches!(e, Event::Copy))
}

pub fn take_cut_event(ctx: &egui::Context) -> bool {
    take_clipboard_event(ctx, |e| matches!(e, Event::Cut))
}

pub fn take_paste_event(ctx: &egui::Context) -> bool {
    take_clipboard_event(ctx, |e| matches!(e, Event::Paste(_)))
}

fn take_clipboard_event(ctx: &egui::Context, want: impl Fn(&Event) -> bool) -> bool {
    ctx.input_mut(|i| {
        let n = i.events.len();
        i.events.retain(|e| !want(e));
        i.events.len() != n
    })
}

impl Serialize for Chord {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_storage())
    }
}

impl<'de> Deserialize<'de> for Chord {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        Chord::parse(&s).ok_or_else(|| serde::de::Error::custom(format!("invalid shortcut: {s}")))
    }
}

fn parse_key(s: &str) -> Option<Key> {
    let want = s.trim();
    Key::ALL.iter().copied().find(|k| {
        k.name().eq_ignore_ascii_case(want)
            || k.symbol_or_name().eq_ignore_ascii_case(want)
            || format!("{k:?}").eq_ignore_ascii_case(want)
    })
}

/// Result of listening for a new shortcut while a settings row is capturing.
pub enum Capture {
    Wait,
    Cancel,
    Bound(Chord),
}

/// If the user pressed a non-modifier key this frame, return it (and consume it).
pub fn take_binding(ctx: &egui::Context) -> Capture {
    let mut found = Capture::Wait;
    ctx.input_mut(|i| {
        if i.consume_key(Modifiers::NONE, Key::Escape) {
            found = Capture::Cancel;
            return;
        }
        let mut bound: Option<Chord> = None;
        for ev in &i.events {
            if let Event::Key {
                key,
                pressed: true,
                modifiers,
                repeat,
                ..
            } = ev
            {
                if *repeat || matches!(*key, Key::Escape | Key::Tab) {
                    continue;
                }
                bound = Some(Chord::from_egui(*modifiers, *key));
                break;
            }
        }
        if let Some(ch) = bound {
            if let Some(k) = ch.key() {
                i.consume_key(ch.modifiers(), k);
            }
            found = Capture::Bound(ch);
        }
    });
    found
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShortcutId {
    OpenTerminal,
    Rename,
    Refresh,
    SwitchPane,
    NewFolder,
    Filter,
    Search,
    Palette,
    Settings,
    NewTab,
    CloseTab,
    EditAddress,
    SelectAll,
    Copy,
    Cut,
    Paste,
    CopyPaths,
    CopyToOther,
    MoveToOther,
    Undo,
    Redo,
    CompareFolders,
    ToggleHidden,
    TogglePreview,
    GoUp,
    HistoryBack,
    HistoryForward,
    Delete,
    DeletePermanent,
    EnterFolder,
    ParentFolder,
}

pub const SHORTCUT_ROWS: &[(ShortcutId, &str)] = &[
    (ShortcutId::OpenTerminal, "Open terminal"),
    (ShortcutId::Rename, "Rename"),
    (ShortcutId::Refresh, "Refresh"),
    (ShortcutId::SwitchPane, "Switch pane"),
    (ShortcutId::NewFolder, "New folder"),
    (ShortcutId::Filter, "Filter"),
    (ShortcutId::Search, "Search"),
    (ShortcutId::Palette, "Quick jump"),
    (ShortcutId::Settings, "Settings"),
    (ShortcutId::NewTab, "New tab"),
    (ShortcutId::CloseTab, "Close tab"),
    (ShortcutId::EditAddress, "Edit address bar"),
    (ShortcutId::SelectAll, "Select all"),
    (ShortcutId::Copy, "Copy"),
    (ShortcutId::Cut, "Cut"),
    (ShortcutId::Paste, "Paste"),
    (ShortcutId::CopyPaths, "Copy path(s)"),
    (ShortcutId::CopyToOther, "Copy to other pane"),
    (ShortcutId::MoveToOther, "Move to other pane"),
    (ShortcutId::Undo, "Undo"),
    (ShortcutId::Redo, "Redo"),
    (ShortcutId::CompareFolders, "Compare folders"),
    (ShortcutId::ToggleHidden, "Toggle hidden files"),
    (ShortcutId::TogglePreview, "Toggle preview pane"),
    (ShortcutId::GoUp, "Go up"),
    (ShortcutId::HistoryBack, "History back"),
    (ShortcutId::HistoryForward, "History forward"),
    (ShortcutId::Delete, "Delete (recycle)"),
    (ShortcutId::DeletePermanent, "Delete permanently"),
    (ShortcutId::EnterFolder, "Enter folder"),
    (ShortcutId::ParentFolder, "Parent folder"),
];

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Keymap {
    pub open_terminal: Chord,
    pub rename: Chord,
    pub refresh: Chord,
    pub switch_pane: Chord,
    pub new_folder: Chord,
    pub filter: Chord,
    pub search: Chord,
    pub palette: Chord,
    pub settings: Chord,
    pub new_tab: Chord,
    pub close_tab: Chord,
    pub edit_address: Chord,
    pub select_all: Chord,
    pub copy: Chord,
    pub cut: Chord,
    pub paste: Chord,
    pub copy_paths: Chord,
    pub copy_to_other: Chord,
    pub move_to_other: Chord,
    pub undo: Chord,
    pub redo: Chord,
    pub compare_folders: Chord,
    pub toggle_hidden: Chord,
    pub toggle_preview: Chord,
    pub go_up: Chord,
    pub history_back: Chord,
    pub history_forward: Chord,
    pub delete: Chord,
    pub delete_permanent: Chord,
    pub enter_folder: Chord,
    pub parent_folder: Chord,
}

impl Default for Keymap {
    fn default() -> Self {
        Self {
            open_terminal: Chord::new(false, false, false, "F1"),
            rename: Chord::new(false, false, false, "F2"),
            refresh: Chord::new(false, false, false, "F5"),
            switch_pane: Chord::new(false, false, false, "F6"),
            new_folder: Chord::new(false, false, false, "F7"),
            filter: Chord::new(true, false, false, "F"),
            search: Chord::new(true, false, true, "F"),
            palette: Chord::new(true, false, false, "P"),
            settings: Chord::new(true, false, false, "Comma"),
            new_tab: Chord::new(true, false, false, "T"),
            close_tab: Chord::new(true, false, false, "W"),
            edit_address: Chord::new(true, false, false, "L"),
            select_all: Chord::new(true, false, false, "A"),
            copy: Chord::new(true, false, false, "C"),
            cut: Chord::new(true, false, false, "X"),
            paste: Chord::new(true, false, false, "V"),
            copy_paths: Chord::new(true, false, true, "C"),
            copy_to_other: Chord::new(true, true, false, "C"),
            move_to_other: Chord::new(true, false, true, "M"),
            undo: Chord::new(true, false, false, "Z"),
            redo: Chord::new(true, false, false, "Y"),
            compare_folders: Chord::new(true, false, false, "D"),
            toggle_hidden: Chord::new(true, false, false, "H"),
            toggle_preview: Chord::new(false, false, false, "Space"),
            go_up: Chord::new(false, false, false, "Backspace"),
            history_back: Chord::new(false, true, false, "ArrowLeft"),
            history_forward: Chord::new(false, true, false, "ArrowRight"),
            delete: Chord::new(false, false, false, "Delete"),
            delete_permanent: Chord::new(false, false, true, "Delete"),
            enter_folder: Chord::unbound(),
            parent_folder: Chord::unbound(),
        }
    }
}

impl Keymap {
    pub fn get(&self, id: ShortcutId) -> &Chord {
        match id {
            ShortcutId::OpenTerminal => &self.open_terminal,
            ShortcutId::Rename => &self.rename,
            ShortcutId::Refresh => &self.refresh,
            ShortcutId::SwitchPane => &self.switch_pane,
            ShortcutId::NewFolder => &self.new_folder,
            ShortcutId::Filter => &self.filter,
            ShortcutId::Search => &self.search,
            ShortcutId::Palette => &self.palette,
            ShortcutId::Settings => &self.settings,
            ShortcutId::NewTab => &self.new_tab,
            ShortcutId::CloseTab => &self.close_tab,
            ShortcutId::EditAddress => &self.edit_address,
            ShortcutId::SelectAll => &self.select_all,
            ShortcutId::Copy => &self.copy,
            ShortcutId::Cut => &self.cut,
            ShortcutId::Paste => &self.paste,
            ShortcutId::CopyPaths => &self.copy_paths,
            ShortcutId::CopyToOther => &self.copy_to_other,
            ShortcutId::MoveToOther => &self.move_to_other,
            ShortcutId::Undo => &self.undo,
            ShortcutId::Redo => &self.redo,
            ShortcutId::CompareFolders => &self.compare_folders,
            ShortcutId::ToggleHidden => &self.toggle_hidden,
            ShortcutId::TogglePreview => &self.toggle_preview,
            ShortcutId::GoUp => &self.go_up,
            ShortcutId::HistoryBack => &self.history_back,
            ShortcutId::HistoryForward => &self.history_forward,
            ShortcutId::Delete => &self.delete,
            ShortcutId::DeletePermanent => &self.delete_permanent,
            ShortcutId::EnterFolder => &self.enter_folder,
            ShortcutId::ParentFolder => &self.parent_folder,
        }
    }

    pub fn get_mut(&mut self, id: ShortcutId) -> &mut Chord {
        match id {
            ShortcutId::OpenTerminal => &mut self.open_terminal,
            ShortcutId::Rename => &mut self.rename,
            ShortcutId::Refresh => &mut self.refresh,
            ShortcutId::SwitchPane => &mut self.switch_pane,
            ShortcutId::NewFolder => &mut self.new_folder,
            ShortcutId::Filter => &mut self.filter,
            ShortcutId::Search => &mut self.search,
            ShortcutId::Palette => &mut self.palette,
            ShortcutId::Settings => &mut self.settings,
            ShortcutId::NewTab => &mut self.new_tab,
            ShortcutId::CloseTab => &mut self.close_tab,
            ShortcutId::EditAddress => &mut self.edit_address,
            ShortcutId::SelectAll => &mut self.select_all,
            ShortcutId::Copy => &mut self.copy,
            ShortcutId::Cut => &mut self.cut,
            ShortcutId::Paste => &mut self.paste,
            ShortcutId::CopyPaths => &mut self.copy_paths,
            ShortcutId::CopyToOther => &mut self.copy_to_other,
            ShortcutId::MoveToOther => &mut self.move_to_other,
            ShortcutId::Undo => &mut self.undo,
            ShortcutId::Redo => &mut self.redo,
            ShortcutId::CompareFolders => &mut self.compare_folders,
            ShortcutId::ToggleHidden => &mut self.toggle_hidden,
            ShortcutId::TogglePreview => &mut self.toggle_preview,
            ShortcutId::GoUp => &mut self.go_up,
            ShortcutId::HistoryBack => &mut self.history_back,
            ShortcutId::HistoryForward => &mut self.history_forward,
            ShortcutId::Delete => &mut self.delete,
            ShortcutId::DeletePermanent => &mut self.delete_permanent,
            ShortcutId::EnterFolder => &mut self.enter_folder,
            ShortcutId::ParentFolder => &mut self.parent_folder,
        }
    }

    pub fn conflicts_with(&self, id: ShortcutId) -> Option<&'static str> {
        let chord = self.get(id);
        for (other, label) in SHORTCUT_ROWS {
            if *other != id && self.get(*other) == chord {
                return Some(*label);
            }
        }
        None
    }

    /// Older builds bound Search to Ctrl+F. If a saved keymap still has that
    /// and Filter is the new Ctrl+F default, move Search to Ctrl+Shift+F.
    pub fn migrate_filter_shortcut(&mut self) {
        let ctrl_f = Chord::new(true, false, false, "F");
        let ctrl_shift_f = Chord::new(true, false, true, "F");
        if self.search == ctrl_f && self.filter == ctrl_f {
            self.search = ctrl_shift_f;
        }
    }

    /// Older builds used ArrowLeft/ArrowRight alone to leave/enter folders.
    /// History is Alt+Arrows; Enter still opens the focused item.
    pub fn migrate_bare_arrow_nav(&mut self) {
        let left = Chord::new(false, false, false, "ArrowLeft");
        let right = Chord::new(false, false, false, "ArrowRight");
        if self.parent_folder == left {
            self.parent_folder = Chord::unbound();
        }
        if self.enter_folder == right {
            self.enter_folder = Chord::unbound();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_labels() {
        let c = Chord::parse("Ctrl+Shift+C").unwrap();
        assert!(c.ctrl && c.shift && !c.alt);
        assert_eq!(c.key, "C");
        assert_eq!(c.label(), "Ctrl+Shift+C");
        assert!(Chord::parse("F7").unwrap().key().is_some());
        assert!(Chord::parse("Alt+ArrowLeft").is_some());
        assert!(Chord::parse("Ctrl+,").is_some());
        assert!(Chord::parse("").unwrap().is_unbound());
    }

    #[test]
    fn defaults_all_resolve() {
        let km = Keymap::default();
        for (id, label) in SHORTCUT_ROWS {
            if km.get(*id).is_unbound() {
                continue;
            }
            assert!(
                km.get(*id).key().is_some(),
                "{label}: {}",
                km.get(*id).to_storage()
            );
        }
    }

    #[test]
    fn toml_roundtrip() {
        let km = Keymap::default();
        let s = toml::to_string(&km).unwrap();
        let back: Keymap = toml::from_str(&s).unwrap();
        assert_eq!(back.new_folder.to_storage(), "F7");
        assert_eq!(back.filter.to_storage(), "Ctrl+F");
        assert_eq!(back.search.to_storage(), "Ctrl+Shift+F");
        assert_eq!(back.copy_paths.to_storage(), "Ctrl+Shift+C");
        assert_eq!(back.settings.to_storage(), "Ctrl+Comma");
    }

    #[test]
    fn migrate_old_ctrl_f_search() {
        let mut km = Keymap::default();
        km.search = Chord::new(true, false, false, "F");
        km.filter = Chord::new(true, false, false, "F");
        km.migrate_filter_shortcut();
        assert_eq!(km.filter.to_storage(), "Ctrl+F");
        assert_eq!(km.search.to_storage(), "Ctrl+Shift+F");
    }

    #[test]
    fn migrate_bare_arrow_nav() {
        let mut km = Keymap::default();
        km.parent_folder = Chord::new(false, false, false, "ArrowLeft");
        km.enter_folder = Chord::new(false, false, false, "ArrowRight");
        km.migrate_bare_arrow_nav();
        assert!(km.parent_folder.is_unbound());
        assert!(km.enter_folder.is_unbound());
    }

    #[test]
    fn copy_chords_match_modifiers() {
        let km = Keymap::default();
        let ctrl = Modifiers::CTRL;
        let ctrl_shift = ctrl | Modifiers::SHIFT;
        let ctrl_alt = ctrl | Modifiers::ALT;
        assert!(km.copy.matches_modifiers(ctrl));
        assert!(!km.copy.matches_modifiers(ctrl_shift));
        assert!(km.copy_paths.matches_modifiers(ctrl_shift));
        assert!(km.copy_to_other.matches_modifiers(ctrl_alt));
        assert!(km.paste.matches_modifiers(ctrl));
        assert!(km.cut.matches_modifiers(ctrl));
        assert!(km.copy.matches_modifiers(Modifiers::COMMAND));
    }
}
