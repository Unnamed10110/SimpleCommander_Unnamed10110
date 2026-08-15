/// Windows file attribute bits we care about (mirrors FILE_ATTRIBUTE_*).
pub const ATTR_READONLY: u32 = 0x0001;
pub const ATTR_HIDDEN: u32 = 0x0002;
pub const ATTR_SYSTEM: u32 = 0x0004;
pub const ATTR_DIRECTORY: u32 = 0x0010;
pub const ATTR_REPARSE_POINT: u32 = 0x0400;

/// A single filesystem entry. Kept intentionally small (name + POD fields)
/// so a 100k-entry directory stays in the tens of megabytes.
#[derive(Clone, Debug)]
pub struct FsEntry {
    pub name: String,
    pub size: u64,
    /// FILETIME (100 ns ticks since 1601-01-01 UTC).
    pub modified: u64,
    /// FILETIME (100 ns ticks since 1601-01-01 UTC).
    pub created: u64,
    pub attributes: u32,
}

impl FsEntry {
    #[inline]
    pub fn is_dir(&self) -> bool {
        self.attributes & ATTR_DIRECTORY != 0
    }

    #[inline]
    pub fn is_hidden(&self) -> bool {
        self.attributes & (ATTR_HIDDEN | ATTR_SYSTEM) != 0
    }

    #[inline]
    pub fn is_reparse(&self) -> bool {
        self.attributes & ATTR_REPARSE_POINT != 0
    }

    /// Extension without the dot. Empty for directories and names without a dot.
    /// Not lowercased — compare with `eq_ignore_ascii_case` if needed.
    #[inline]
    pub fn ext(&self) -> &str {
        if self.is_dir() {
            return "";
        }
        match self.name.rfind('.') {
            Some(i) if i > 0 && i + 1 < self.name.len() => &self.name[i + 1..],
            _ => "",
        }
    }
}

/// Convert a FILETIME tick count to unix seconds (may be negative for pre-1970).
pub fn filetime_to_unix_secs(ft: u64) -> i64 {
    const EPOCH_DIFF_SECS: i64 = 11_644_473_600;
    (ft / 10_000_000) as i64 - EPOCH_DIFF_SECS
}

/// Human-readable size, XYplorer style ("1.23 MB").
pub fn format_size(bytes: u64) -> String {
    const UNITS: [&str; 6] = ["B", "KB", "MB", "GB", "TB", "PB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut v = bytes as f64;
    let mut unit = 0;
    while v >= 1024.0 && unit < UNITS.len() - 1 {
        v /= 1024.0;
        unit += 1;
    }
    if v >= 100.0 {
        format!("{v:.0} {}", UNITS[unit])
    } else if v >= 10.0 {
        format!("{v:.1} {}", UNITS[unit])
    } else {
        format!("{v:.2} {}", UNITS[unit])
    }
}
