//! Fast directory enumeration on the raw Win32 API.
//!
//! Uses `FindFirstFileExW` with `FindExInfoBasic` (skips short-name lookup)
//! and `FIND_FIRST_EX_LARGE_FETCH` (larger kernel buffers), which is the
//! fastest documented user-mode enumeration path on Windows.

use sc_core::FsEntry;
use std::path::Path;
use windows::core::PCWSTR;
use windows::Win32::Foundation::ERROR_NO_MORE_FILES;
use windows::Win32::Foundation::FILETIME;
use windows::Win32::Storage::FileSystem::{
    FileTimeToLocalFileTime, FindClose, FindExInfoBasic, FindExSearchNameMatch, FindFirstFileExW,
    FindNextFileW, FIND_FIRST_EX_LARGE_FETCH, WIN32_FIND_DATAW,
};

pub const BATCH_SIZE: usize = 4096;

/// Convert a UTC FILETIME tick count to the equivalent local FILETIME.
/// Display code can then format those ticks as civil wall-clock time.
pub fn filetime_utc_to_local(ft: u64) -> u64 {
    if ft == 0 {
        return 0;
    }
    let utc = FILETIME {
        dwLowDateTime: ft as u32,
        dwHighDateTime: (ft >> 32) as u32,
    };
    let mut local = FILETIME::default();
    unsafe {
        if FileTimeToLocalFileTime(&utc, &mut local).is_ok() {
            ((local.dwHighDateTime as u64) << 32) | local.dwLowDateTime as u64
        } else {
            ft
        }
    }
}

/// Convert a path to an extended-length wide string with a trailing pattern.
fn wide_search_pattern(path: &Path) -> Vec<u16> {
    let mut s = path.as_os_str().to_os_string();
    let raw = s.to_string_lossy();
    let mut out = String::new();
    if raw.starts_with("\\\\?\\") {
        out.push_str(&raw);
    } else if raw.starts_with("\\\\") {
        // UNC path -> \\?\UNC\server\share...
        out.push_str("\\\\?\\UNC\\");
        out.push_str(&raw[2..]);
    } else {
        out.push_str("\\\\?\\");
        out.push_str(&raw);
    }
    if !out.ends_with('\\') {
        out.push('\\');
    }
    out.push('*');
    s = out.into();
    let mut w: Vec<u16> = s.encode_wide().collect();
    w.push(0);
    w
}

use std::os::windows::ffi::OsStrExt;

#[inline]
fn find_data_to_entry(fd: &WIN32_FIND_DATAW) -> Option<FsEntry> {
    let len = fd.cFileName.iter().position(|&c| c == 0).unwrap_or(fd.cFileName.len());
    let name_utf16 = &fd.cFileName[..len];
    // Skip "." and ".."
    if len == 0
        || (name_utf16[0] == b'.' as u16
            && (len == 1 || (len == 2 && name_utf16[1] == b'.' as u16)))
    {
        return None;
    }
    let name = String::from_utf16_lossy(name_utf16);
    Some(FsEntry {
        name,
        size: ((fd.nFileSizeHigh as u64) << 32) | fd.nFileSizeLow as u64,
        modified: ((fd.ftLastWriteTime.dwHighDateTime as u64) << 32)
            | fd.ftLastWriteTime.dwLowDateTime as u64,
        created: ((fd.ftCreationTime.dwHighDateTime as u64) << 32)
            | fd.ftCreationTime.dwLowDateTime as u64,
        attributes: fd.dwFileAttributes,
    })
}

/// Enumerate a directory, streaming entries to `on_batch` in chunks of
/// [`BATCH_SIZE`]. Return `false` from the callback to cancel.
pub fn enumerate_dir(
    path: &Path,
    mut on_batch: impl FnMut(Vec<FsEntry>) -> bool,
) -> Result<(), String> {
    let pattern = wide_search_pattern(path);
    let mut fd = WIN32_FIND_DATAW::default();
    let handle = unsafe {
        FindFirstFileExW(
            PCWSTR::from_raw(pattern.as_ptr()),
            FindExInfoBasic,
            &mut fd as *mut _ as *mut _,
            FindExSearchNameMatch,
            None,
            FIND_FIRST_EX_LARGE_FETCH,
        )
    }
    .map_err(|e| format!("{}: {}", path.display(), e.message()))?;

    let mut batch = Vec::with_capacity(BATCH_SIZE);
    let mut result = Ok(());
    loop {
        if let Some(entry) = find_data_to_entry(&fd) {
            batch.push(entry);
            if batch.len() >= BATCH_SIZE {
                if !on_batch(std::mem::replace(&mut batch, Vec::with_capacity(BATCH_SIZE))) {
                    break;
                }
            }
        }
        if let Err(e) = unsafe { FindNextFileW(handle, &mut fd) } {
            if e.code() != ERROR_NO_MORE_FILES.to_hresult() {
                result = Err(e.message().to_string());
            }
            break;
        }
    }
    if !batch.is_empty() {
        on_batch(batch);
    }
    unsafe {
        let _ = FindClose(handle);
    }
    result
}

/// Recursively enumerate a directory tree. Entry names are reported as paths
/// relative to `root` (used by flatten view and the fallback index).
/// `on_batch` receives (relative_dir, entries); return `false` to cancel.
pub fn enumerate_tree(
    root: &Path,
    on_batch: &mut dyn FnMut(&Path, Vec<FsEntry>) -> bool,
) -> Result<(), String> {
    let mut stack: Vec<std::path::PathBuf> = vec![std::path::PathBuf::new()];
    while let Some(rel) = stack.pop() {
        let abs = root.join(&rel);
        let mut cancelled = false;
        let res = enumerate_dir(&abs, |batch| {
            for e in &batch {
                if e.is_dir() && !e.is_reparse() {
                    stack.push(rel.join(&e.name));
                }
            }
            if !on_batch(&rel, batch) {
                cancelled = true;
                return false;
            }
            true
        });
        if cancelled {
            return Ok(());
        }
        // Ignore per-subdir errors (access denied etc.); keep walking.
        if rel.as_os_str().is_empty() {
            res?;
        }
    }
    Ok(())
}

/// Compute the total size of a directory tree (parallel-friendly, cancellable
/// via the returned closure contract: caller polls a flag inside `should_stop`).
pub fn dir_size(root: &Path, should_stop: &dyn Fn() -> bool) -> u64 {
    let mut total = 0u64;
    let _ = enumerate_tree(root, &mut |_rel, batch| {
        for e in &batch {
            total += e.size;
        }
        !should_stop()
    });
    total
}
