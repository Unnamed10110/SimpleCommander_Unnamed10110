//! Explorer-compatible file clipboard: CF_HDROP plus the
//! "Preferred DropEffect" format so cut/copy round-trips with Explorer.

use std::path::{Path, PathBuf};
use windows::core::PCWSTR;
use windows::Win32::Foundation::{HANDLE, HGLOBAL, HWND, POINT};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, GetClipboardData, OpenClipboard,
    RegisterClipboardFormatW, SetClipboardData,
};
use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};
use windows::Win32::System::Ole::CF_HDROP;
use windows::Win32::UI::Shell::{DragQueryFileW, HDROP};

use std::os::windows::ffi::OsStrExt;

const DROPEFFECT_COPY: u32 = 1;
const DROPEFFECT_MOVE: u32 = 2;

#[repr(C)]
struct DropFiles {
    p_files: u32,
    pt: POINT,
    f_nc: i32,
    f_wide: i32,
}

fn preferred_drop_effect_format() -> u32 {
    let name: Vec<u16> = "Preferred DropEffect\0".encode_utf16().collect();
    unsafe { RegisterClipboardFormatW(PCWSTR::from_raw(name.as_ptr())) }
}

/// Put files on the OS clipboard (Explorer-compatible). `cut` marks them for move.
pub fn set_clipboard_files(paths: &[PathBuf], cut: bool) -> Result<(), String> {
    // Build the double-null-terminated wide path list.
    let mut list: Vec<u16> = Vec::new();
    for p in paths {
        list.extend(p.as_os_str().encode_wide());
        list.push(0);
    }
    list.push(0);
    let header = std::mem::size_of::<DropFiles>();
    let total = header + list.len() * 2;

    unsafe {
        OpenClipboard(None).map_err(|e| e.message().to_string())?;
        let result = (|| {
            EmptyClipboard().map_err(|e| e.message().to_string())?;

            let hmem = GlobalAlloc(GMEM_MOVEABLE, total).map_err(|e| e.message().to_string())?;
            let ptr = GlobalLock(hmem) as *mut u8;
            if ptr.is_null() {
                return Err("GlobalLock failed".into());
            }
            let df = ptr as *mut DropFiles;
            (*df).p_files = header as u32;
            (*df).pt = POINT::default();
            (*df).f_nc = 0;
            (*df).f_wide = 1;
            std::ptr::copy_nonoverlapping(
                list.as_ptr() as *const u8,
                ptr.add(header),
                list.len() * 2,
            );
            let _ = GlobalUnlock(hmem);
            SetClipboardData(CF_HDROP.0 as u32, Some(HANDLE(hmem.0)))
                .map_err(|e| e.message().to_string())?;

            // Preferred DropEffect (copy or move).
            let heffect =
                GlobalAlloc(GMEM_MOVEABLE, 4).map_err(|e| e.message().to_string())?;
            let eptr = GlobalLock(heffect) as *mut u32;
            if !eptr.is_null() {
                *eptr = if cut { DROPEFFECT_MOVE } else { DROPEFFECT_COPY };
                let _ = GlobalUnlock(heffect);
            }
            SetClipboardData(preferred_drop_effect_format(), Some(HANDLE(heffect.0)))
                .map_err(|e| e.message().to_string())?;
            Ok(())
        })();
        let _ = CloseClipboard();
        result
    }
}

/// Read files from the OS clipboard; returns (paths, is_cut).
pub fn get_clipboard_files() -> Option<(Vec<PathBuf>, bool)> {
    unsafe {
        OpenClipboard(Some(HWND::default())).ok()?;
        let result = (|| {
            let handle = GetClipboardData(CF_HDROP.0 as u32).ok()?;
            let hdrop = HDROP(handle.0);
            let count = DragQueryFileW(hdrop, u32::MAX, None);
            let mut paths = Vec::with_capacity(count as usize);
            for i in 0..count {
                let len = DragQueryFileW(hdrop, i, None);
                let mut buf = vec![0u16; len as usize + 1];
                let got = DragQueryFileW(hdrop, i, Some(&mut buf));
                if got > 0 {
                    paths.push(PathBuf::from(String::from_utf16_lossy(&buf[..got as usize])));
                }
            }
            let mut cut = false;
            if let Ok(heffect) = GetClipboardData(preferred_drop_effect_format()) {
                let hg = HGLOBAL(heffect.0);
                let ptr = GlobalLock(hg) as *const u32;
                if !ptr.is_null() {
                    cut = *ptr & DROPEFFECT_MOVE != 0;
                    let _ = GlobalUnlock(hg);
                }
            }
            Some((paths, cut))
        })();
        let _ = CloseClipboard();
        result
    }
}

/// True if the clipboard currently holds files.
pub fn clipboard_has_files() -> bool {
    unsafe {
        use windows::Win32::System::DataExchange::IsClipboardFormatAvailable;
        IsClipboardFormatAvailable(CF_HDROP.0 as u32).is_ok()
    }
}

/// Copy a text string to the clipboard (used by "Copy path").
pub fn set_clipboard_text(text: &str) -> Result<(), String> {
    let wide: Vec<u16> = std::ffi::OsStr::new(text)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    unsafe {
        OpenClipboard(None).map_err(|e| e.message().to_string())?;
        let result = (|| {
            EmptyClipboard().map_err(|e| e.message().to_string())?;
            let hmem =
                GlobalAlloc(GMEM_MOVEABLE, wide.len() * 2).map_err(|e| e.message().to_string())?;
            let ptr = GlobalLock(hmem) as *mut u16;
            if ptr.is_null() {
                return Err("GlobalLock failed".into());
            }
            std::ptr::copy_nonoverlapping(wide.as_ptr(), ptr, wide.len());
            let _ = GlobalUnlock(hmem);
            const CF_UNICODETEXT: u32 = 13;
            SetClipboardData(CF_UNICODETEXT, Some(HANDLE(hmem.0)))
                .map_err(|e| e.message().to_string())?;
            Ok(())
        })();
        let _ = CloseClipboard();
        result
    }
}

/// Ensure `Path` import is considered used on all feature paths.
#[allow(dead_code)]
fn _p(_: &Path) {}
