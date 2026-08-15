//! Drive/volume discovery and known-folder helpers for the navigation
//! sidebar and breadcrumb root menu.

use std::path::PathBuf;
use windows::core::PCWSTR;
use windows::Win32::Storage::FileSystem::{
    GetDiskFreeSpaceExW, GetDriveTypeW, GetLogicalDrives, GetVolumeInformationW,
};

use std::os::windows::ffi::OsStrExt;

#[derive(Clone, Debug)]
pub struct VolumeInfo {
    pub root: PathBuf,
    pub label: String,
    pub drive_type: DriveType,
    pub total_bytes: u64,
    pub free_bytes: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DriveType {
    Fixed,
    Removable,
    Network,
    CdRom,
    Ram,
    Unknown,
}

/// Enumerate mounted drive letters with labels and capacity.
pub fn list_volumes() -> Vec<VolumeInfo> {
    let mask = unsafe { GetLogicalDrives() };
    let mut out = Vec::new();
    for i in 0..26u32 {
        if mask & (1 << i) == 0 {
            continue;
        }
        let letter = (b'A' + i as u8) as char;
        let root = format!("{letter}:\\");
        let wide: Vec<u16> = std::ffi::OsStr::new(&root)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let dtype = match unsafe { GetDriveTypeW(PCWSTR::from_raw(wide.as_ptr())) } {
            2 => DriveType::Removable,
            3 => DriveType::Fixed,
            4 => DriveType::Network,
            5 => DriveType::CdRom,
            6 => DriveType::Ram,
            _ => DriveType::Unknown,
        };
        let mut label_buf = [0u16; 261];
        let label = unsafe {
            GetVolumeInformationW(
                PCWSTR::from_raw(wide.as_ptr()),
                Some(&mut label_buf),
                None,
                None,
                None,
                None,
            )
        }
        .ok()
        .map(|_| {
            let len = label_buf.iter().position(|&c| c == 0).unwrap_or(0);
            String::from_utf16_lossy(&label_buf[..len])
        })
        .unwrap_or_default();
        let mut free = 0u64;
        let mut total = 0u64;
        unsafe {
            let _ = GetDiskFreeSpaceExW(
                PCWSTR::from_raw(wide.as_ptr()),
                None,
                Some(&mut total),
                Some(&mut free),
            );
        }
        out.push(VolumeInfo {
            root: PathBuf::from(root),
            label,
            drive_type: dtype,
            total_bytes: total,
            free_bytes: free,
        });
    }
    out
}

/// Best matching mounted volume for `path` (drive letter or longest root prefix).
pub fn volume_for_path<'a>(volumes: &'a [VolumeInfo], path: &std::path::Path) -> Option<&'a VolumeInfo> {
    let needle = path.to_string_lossy().to_ascii_uppercase();
    volumes
        .iter()
        .filter(|v| {
            let root = v.root.to_string_lossy().to_ascii_uppercase();
            needle.starts_with(root.trim_end_matches('\\'))
        })
        .max_by_key(|v| v.root.as_os_str().len())
}

/// Common user folders for the sidebar (only existing ones are returned).
pub fn known_folders() -> Vec<(String, PathBuf)> {
    let home = sc_core::state::dirs_home();
    let candidates = [
        ("Home", home.clone()),
        ("Desktop", home.join("Desktop")),
        ("Documents", home.join("Documents")),
        ("Downloads", home.join("Downloads")),
        ("Pictures", home.join("Pictures")),
        ("Music", home.join("Music")),
        ("Videos", home.join("Videos")),
    ];
    candidates
        .into_iter()
        .filter(|(_, p)| p.is_dir())
        .map(|(n, p)| (n.to_string(), p))
        .collect()
}
