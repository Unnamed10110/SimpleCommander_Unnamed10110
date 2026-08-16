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

/// WSL distros visible under `\\wsl$` / `\\wsl.localhost`.
pub fn wsl_distros() -> Vec<(String, PathBuf)> {
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for root in [r"\\wsl.localhost", r"\\wsl$"] {
        let rd = match std::fs::read_dir(root) {
            Ok(rd) => rd,
            Err(_) => continue,
        };
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if name.is_empty() || name.starts_with('.') {
                continue;
            }
            let is_dir = e.file_type().map(|t| t.is_dir()).unwrap_or(true);
            if !is_dir {
                continue;
            }
            if seen.insert(name.to_ascii_lowercase()) {
                out.push((name, e.path()));
            }
        }
        if !out.is_empty() {
            break;
        }
    }
    out.sort_by(|a, b| a.0.to_ascii_lowercase().cmp(&b.0.to_ascii_lowercase()));
    out
}

/// Connected and remembered network shares (UNC roots).
pub fn network_places() -> Vec<(String, PathBuf)> {
    use windows::Win32::NetworkManagement::WNet::{RESOURCE_CONNECTED, RESOURCE_REMEMBERED};
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for scope in [RESOURCE_CONNECTED, RESOURCE_REMEMBERED] {
        enum_net_scope(scope, &mut out, &mut seen);
    }
    out.sort_by(|a, b| a.0.to_ascii_lowercase().cmp(&b.0.to_ascii_lowercase()));
    out
}

fn enum_net_scope(
    scope: windows::Win32::NetworkManagement::WNet::NET_RESOURCE_SCOPE,
    out: &mut Vec<(String, PathBuf)>,
    seen: &mut std::collections::HashSet<String>,
) {
    use windows::Win32::Foundation::{ERROR_NO_MORE_ITEMS, HANDLE};
    use windows::Win32::NetworkManagement::WNet::{
        WNetCloseEnum, WNetEnumResourceW, WNetOpenEnumW, NETRESOURCEW, RESOURCETYPE_DISK,
        RESOURCEUSAGE_CONNECTABLE,
    };
    unsafe {
        let mut handle = HANDLE::default();
        if WNetOpenEnumW(
            scope,
            RESOURCETYPE_DISK,
            RESOURCEUSAGE_CONNECTABLE,
            None,
            &mut handle,
        )
        .is_err()
        {
            return;
        }
        let mut buf = vec![0u8; 16 * 1024];
        loop {
            let mut count = u32::MAX;
            let mut size = buf.len() as u32;
            let err = WNetEnumResourceW(
                handle,
                &mut count,
                buf.as_mut_ptr() as *mut _,
                &mut size,
            );
            if err == ERROR_NO_MORE_ITEMS {
                break;
            }
            if err.is_err() {
                if size as usize > buf.len() {
                    buf.resize(size as usize, 0);
                    continue;
                }
                break;
            }
            if count == 0 {
                break;
            }
            let recs = std::slice::from_raw_parts(buf.as_ptr() as *const NETRESOURCEW, count as usize);
            for r in recs {
                let remote = pwstr_to_string(r.lpRemoteName);
                if remote.is_empty() {
                    continue;
                }
                let key = remote.to_ascii_lowercase();
                if !seen.insert(key) {
                    continue;
                }
                let comment = pwstr_to_string(r.lpComment);
                let label = if comment.is_empty() {
                    remote.clone()
                } else {
                    format!("{remote} ({comment})")
                };
                out.push((label, PathBuf::from(remote)));
            }
        }
        let _ = WNetCloseEnum(handle);
    }
}

fn pwstr_to_string(p: windows::core::PWSTR) -> String {
    if p.is_null() {
        return String::new();
    }
    unsafe { p.to_string().unwrap_or_default() }
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
