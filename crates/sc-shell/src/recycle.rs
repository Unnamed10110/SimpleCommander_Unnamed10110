//! Recycle Bin as a virtual folder (`recycle:\`), enumerated via the shell
//! Recycle Bin IShellFolder / IShellItem APIs rather than `$Recycle.Bin`.

use sc_core::entry::{FsEntry, ATTR_DIRECTORY};
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use windows::core::{Interface, GUID, PCWSTR, PWSTR};
use windows::Win32::Foundation::{HWND, PROPERTYKEY};
use windows::Win32::System::Com::{CoCreateInstance, CoTaskMemFree, CLSCTX_ALL};
use windows::Win32::System::SystemServices::SFGAO_FOLDER;
use windows::Win32::UI::Shell::{
    FileOperation, IEnumShellItems, IFileOperation, IShellItem, IShellItem2,
    SHCreateItemFromParsingName, SHGetKnownFolderItem, BHID_EnumItems, FOF_NOCONFIRMATION,
    FOF_NO_UI, FOLDERID_RecycleBinFolder, KF_FLAG_DEFAULT, SIGDN_DESKTOPABSOLUTEPARSING,
    SIGDN_NORMALDISPLAY,
};

/// Sentinel path used by tabs that show the Recycle Bin.
pub const RECYCLE_PATH: &str = r"recycle:\";

pub fn recycle_root() -> PathBuf {
    PathBuf::from(RECYCLE_PATH)
}

pub fn is_recycle_path(p: &Path) -> bool {
    let s = p.to_string_lossy();
    let t = s.trim_end_matches(['\\', '/']);
    t.eq_ignore_ascii_case("recycle:") || t.eq_ignore_ascii_case("recycle")
}

#[derive(Clone, Debug)]
pub struct RecycleItem {
    pub name: String,
    pub original_path: Option<PathBuf>,
    pub size: u64,
    pub deleted: u64,
    pub is_dir: bool,
    pub parsing_name: String,
}

impl RecycleItem {
    pub fn to_entry(&self) -> FsEntry {
        FsEntry {
            name: self.name.clone(),
            size: self.size,
            modified: self.deleted,
            created: self.deleted,
            attributes: if self.is_dir { ATTR_DIRECTORY } else { 0 },
        }
    }
}

// System.Recycle.DeletedFrom / DateDeleted
const FMTID_RECYCLE: GUID = GUID::from_u128(0x9B174B33_40FF_11D2_A27E_00C04FC30871);
const PKEY_DELETED_FROM: PROPERTYKEY = PROPERTYKEY {
    fmtid: FMTID_RECYCLE,
    pid: 2,
};
const PKEY_DATE_DELETED: PROPERTYKEY = PROPERTYKEY {
    fmtid: FMTID_RECYCLE,
    pid: 3,
};
// System.Size
const PKEY_SIZE: PROPERTYKEY = PROPERTYKEY {
    fmtid: GUID::from_u128(0xB725F130_47EF_101A_A5F1_02608C9EEBAC),
    pid: 12,
};

fn wide(s: &str) -> Vec<u16> {
    std::ffi::OsStr::new(s)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

fn item_name(item: &IShellItem, sigdn: windows::Win32::UI::Shell::SIGDN) -> Option<String> {
    unsafe {
        let pw: PWSTR = item.GetDisplayName(sigdn).ok()?;
        let s = pw.to_string().ok();
        CoTaskMemFree(Some(pw.0 as *const _));
        s
    }
}

fn prop_string(item: &IShellItem2, key: &PROPERTYKEY) -> Option<String> {
    unsafe {
        let pw: PWSTR = item.GetString(key).ok()?;
        let s = pw.to_string().ok();
        CoTaskMemFree(Some(pw.0 as *const _));
        s
    }
}

/// Enumerate Recycle Bin items via `FOLDERID_RecycleBinFolder`.
pub fn list_recycle() -> Result<Vec<RecycleItem>, String> {
    unsafe {
        let folder: IShellItem =
            SHGetKnownFolderItem(&FOLDERID_RecycleBinFolder, KF_FLAG_DEFAULT, None)
                .map_err(|e| e.message().to_string())?;
        let enumerator: IEnumShellItems = folder
            .BindToHandler(None, &BHID_EnumItems)
            .map_err(|e| e.message().to_string())?;
        let mut out = Vec::new();
        let mut used_names: std::collections::HashSet<String> = std::collections::HashSet::new();
        loop {
            let mut slot: [Option<IShellItem>; 1] = [None];
            let mut fetched = 0u32;
            if enumerator.Next(&mut slot, Some(&mut fetched)).is_err() || fetched == 0 {
                break;
            }
            let Some(item) = slot[0].take() else { continue };
            let Some(parsing_name) = item_name(&item, SIGDN_DESKTOPABSOLUTEPARSING) else {
                continue;
            };
            let mut name = item_name(&item, SIGDN_NORMALDISPLAY)
                .unwrap_or_else(|| parsing_name.clone());
            if !used_names.insert(name.clone()) {
                let mut n = 2;
                loop {
                    let candidate = format!("{name} ({n})");
                    if used_names.insert(candidate.clone()) {
                        name = candidate;
                        break;
                    }
                    n += 1;
                }
            }
            let item2: Option<IShellItem2> = item.cast().ok();
            let original_path = item2
                .as_ref()
                .and_then(|i| prop_string(i, &PKEY_DELETED_FROM))
                .map(PathBuf::from);
            let size = item2
                .as_ref()
                .and_then(|i| i.GetUInt64(&PKEY_SIZE).ok())
                .unwrap_or(0);
            let deleted = item2
                .as_ref()
                .and_then(|i| {
                    let ft = i.GetFileTime(&PKEY_DATE_DELETED).ok()?;
                    Some(((ft.dwHighDateTime as u64) << 32) | ft.dwLowDateTime as u64)
                })
                .unwrap_or(0);
            let is_dir = item
                .GetAttributes(SFGAO_FOLDER)
                .ok()
                .map(|f| f.0 & SFGAO_FOLDER.0 != 0)
                .unwrap_or(false);
            out.push(RecycleItem {
                name,
                original_path,
                size,
                deleted,
                is_dir,
                parsing_name,
            });
        }
        Ok(out)
    }
}

fn items_from_parsing_names(names: &[String]) -> Result<Vec<IShellItem>, String> {
    let mut items = Vec::with_capacity(names.len());
    for n in names {
        let w = wide(n);
        unsafe {
            let item: IShellItem = SHCreateItemFromParsingName(PCWSTR::from_raw(w.as_ptr()), None)
                .map_err(|e| format!("{n}: {}", e.message()))?;
            items.push(item);
        }
    }
    Ok(items)
}

/// Restore Recycle Bin items to their original locations.
pub fn restore_items(parsing_names: &[String]) -> Result<(), String> {
    if parsing_names.is_empty() {
        return Ok(());
    }
    let items = items_from_parsing_names(parsing_names)?;
    unsafe {
        let op: IFileOperation = CoCreateInstance(&FileOperation, None, CLSCTX_ALL)
            .map_err(|e| e.message().to_string())?;
        op.SetOperationFlags(FOF_NOCONFIRMATION | FOF_NO_UI)
            .map_err(|e| e.message().to_string())?;
        op.SetOwnerWindow(HWND::default()).ok();
        for item in &items {
            let item2: IShellItem2 = item.cast().map_err(|e| e.message().to_string())?;
            let orig = prop_string(&item2, &PKEY_DELETED_FROM)
                .ok_or_else(|| "item has no original path".to_string())?;
            let orig_path = PathBuf::from(&orig);
            let parent = orig_path
                .parent()
                .ok_or_else(|| format!("no parent for {orig}"))?;
            let parent_w = wide(&parent.to_string_lossy());
            let dest: IShellItem =
                SHCreateItemFromParsingName(PCWSTR::from_raw(parent_w.as_ptr()), None)
                    .map_err(|e| e.message().to_string())?;
            let fname = orig_path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            let fname_w = wide(&fname);
            op.MoveItem(
                item,
                &dest,
                PCWSTR::from_raw(fname_w.as_ptr()),
                None,
            )
            .map_err(|e| e.message().to_string())?;
        }
        op.PerformOperations()
            .map_err(|e| e.message().to_string())?;
    }
    Ok(())
}

/// Permanently delete Recycle Bin items (no undo).
pub fn delete_permanent(parsing_names: &[String]) -> Result<(), String> {
    if parsing_names.is_empty() {
        return Ok(());
    }
    let items = items_from_parsing_names(parsing_names)?;
    unsafe {
        let op: IFileOperation = CoCreateInstance(&FileOperation, None, CLSCTX_ALL)
            .map_err(|e| e.message().to_string())?;
        op.SetOperationFlags(FOF_NOCONFIRMATION | FOF_NO_UI)
            .map_err(|e| e.message().to_string())?;
        op.SetOwnerWindow(HWND::default()).ok();
        for item in &items {
            op.DeleteItem(item, None)
                .map_err(|e| e.message().to_string())?;
        }
        op.PerformOperations()
            .map_err(|e| e.message().to_string())?;
    }
    Ok(())
}

