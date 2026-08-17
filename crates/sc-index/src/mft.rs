//! NTFS master-file-table enumeration via `FSCTL_ENUM_USN_DATA` and live
//! updates via USN-journal tailing. This is the same technique Everything
//! uses: the whole volume's filenames are read in seconds without touching
//! directories. Requires an elevated process; callers fall back to
//! [`crate::fallback`] when volume access is denied.

use parking_lot::RwLock;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use windows::core::PCWSTR;
use windows::Win32::Foundation::{CloseHandle, HANDLE, GENERIC_READ};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION,
    FILE_ATTRIBUTE_DIRECTORY, FILE_FLAG_BACKUP_SEMANTICS, FILE_SHARE_READ, FILE_SHARE_WRITE,
    OPEN_EXISTING,
};
use windows::Win32::System::Ioctl::{
    FSCTL_ENUM_USN_DATA, FSCTL_QUERY_USN_JOURNAL, FSCTL_READ_USN_JOURNAL, MFT_ENUM_DATA_V0,
    READ_USN_JOURNAL_DATA_V0, USN_JOURNAL_DATA_V0, USN_RECORD_V2,
};
use windows::Win32::System::IO::DeviceIoControl;

use std::os::windows::ffi::OsStrExt;

const USN_REASON_FILE_CREATE: u32 = 0x0000_0100;
const USN_REASON_FILE_DELETE: u32 = 0x0000_0200;
const USN_REASON_RENAME_OLD_NAME: u32 = 0x0000_1000;
const USN_REASON_RENAME_NEW_NAME: u32 = 0x0000_2000;

struct IndexEntry {
    parent: u64,
    name: Box<str>,
    name_lower: Box<str>,
    is_dir: bool,
}

struct Inner {
    /// FRN -> entry.
    entries: HashMap<u64, IndexEntry>,
    root_frn: u64,
}

/// In-memory filename index of one NTFS volume.
pub struct MftIndex {
    drive: char,
    inner: RwLock<Inner>,
    stop: AtomicBool,
}

struct VolumeHandle(HANDLE);
impl Drop for VolumeHandle {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}
unsafe impl Send for VolumeHandle {}

fn open_volume(drive: char) -> Result<VolumeHandle, String> {
    let path = format!("\\\\.\\{drive}:");
    let wide: Vec<u16> = std::ffi::OsStr::new(&path)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let h = unsafe {
        CreateFileW(
            PCWSTR::from_raw(wide.as_ptr()),
            GENERIC_READ.0,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            None,
            OPEN_EXISTING,
            Default::default(),
            None,
        )
    }
    .map_err(|e| format!("open volume {drive}: {}", e.message()))?;
    Ok(VolumeHandle(h))
}

/// Get the file reference number of the volume root ("C:\").
fn root_frn(drive: char) -> Result<u64, String> {
    let path = format!("{drive}:\\");
    let wide: Vec<u16> = std::ffi::OsStr::new(&path)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let h = unsafe {
        CreateFileW(
            PCWSTR::from_raw(wide.as_ptr()),
            0,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            None,
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS,
            None,
        )
    }
    .map_err(|e| e.message().to_string())?;
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    let res = unsafe { GetFileInformationByHandle(h, &mut info) };
    unsafe {
        let _ = CloseHandle(h);
    }
    res.map_err(|e| e.message().to_string())?;
    Ok(((info.nFileIndexHigh as u64) << 32) | info.nFileIndexLow as u64)
}

/// Parse a buffer of USN_RECORD_V2 entries, calling `f(record)` for each.
unsafe fn for_each_record(buf: &[u8], mut f: impl FnMut(&USN_RECORD_V2)) {
    let mut offset = 0usize;
    while offset + std::mem::size_of::<USN_RECORD_V2>() <= buf.len() {
        let rec = unsafe { &*(buf.as_ptr().add(offset) as *const USN_RECORD_V2) };
        let len = rec.RecordLength as usize;
        if len == 0 || offset + len > buf.len() {
            break;
        }
        if rec.MajorVersion == 2 {
            f(rec);
        }
        offset += len;
    }
}

unsafe fn record_name(rec: &USN_RECORD_V2) -> String {
    let base = rec as *const USN_RECORD_V2 as *const u8;
    let name_ptr = unsafe { base.add(rec.FileNameOffset as usize) } as *const u16;
    let name_len = rec.FileNameLength as usize / 2;
    let slice = unsafe { std::slice::from_raw_parts(name_ptr, name_len) };
    String::from_utf16_lossy(slice)
}

impl MftIndex {
    /// Enumerate the volume's MFT. Blocking; run on a background thread.
    pub fn build(drive: char) -> Result<Arc<Self>, String> {
        let vol = open_volume(drive)?;
        let root = root_frn(drive)?;
        let mut entries: HashMap<u64, IndexEntry> = HashMap::with_capacity(1 << 18);

        let mut enum_data = MFT_ENUM_DATA_V0 {
            StartFileReferenceNumber: 0,
            LowUsn: 0,
            HighUsn: i64::MAX,
        };
        let mut buf = vec![0u8; 1 << 20]; // 1 MiB per DeviceIoControl round-trip
        loop {
            let mut returned = 0u32;
            let ok = unsafe {
                DeviceIoControl(
                    vol.0,
                    FSCTL_ENUM_USN_DATA,
                    Some(&enum_data as *const _ as *const _),
                    std::mem::size_of::<MFT_ENUM_DATA_V0>() as u32,
                    Some(buf.as_mut_ptr() as *mut _),
                    buf.len() as u32,
                    Some(&mut returned),
                    None,
                )
            };
            if ok.is_err() || returned < 8 {
                break; // ERROR_HANDLE_EOF ends the enumeration
            }
            enum_data.StartFileReferenceNumber =
                u64::from_le_bytes(buf[..8].try_into().unwrap());
            unsafe {
                for_each_record(&buf[8..returned as usize], |rec| {
                    let name = record_name(rec);
                    entries.insert(
                        rec.FileReferenceNumber,
                        IndexEntry {
                            parent: rec.ParentFileReferenceNumber,
                            name_lower: name.to_lowercase().into_boxed_str(),
                            name: name.into_boxed_str(),
                            is_dir: rec.FileAttributes & FILE_ATTRIBUTE_DIRECTORY.0 != 0,
                        },
                    );
                });
            }
        }
        if entries.is_empty() {
            return Err(format!("MFT enumeration returned no records for {drive}:"));
        }

        let index = Arc::new(Self {
            drive,
            inner: RwLock::new(Inner { entries, root_frn: root }),
            stop: AtomicBool::new(false),
        });
        index.clone().spawn_usn_tail();
        Ok(index)
    }

    pub fn len(&self) -> usize {
        self.inner.read().entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }

    /// Case-insensitive Everything-style search; `pred` sees a full path.
    pub fn search(&self, pred: &dyn Fn(&str) -> bool, name_suffix: Option<&str>, max: usize, dirs_only: bool, out: &mut Vec<(PathBuf, bool)>) {
        let inner = self.inner.read();
        let mut path_cache: HashMap<u64, Option<String>> = HashMap::new();
        for (_frn, e) in inner.entries.iter() {
            if out.len() >= max {
                return;
            }
            if dirs_only && !e.is_dir {
                continue;
            }
            if let Some(suf) = name_suffix {
                if !e.name_lower.ends_with(suf) {
                    continue;
                }
            }
            if let Some(dir) = resolve_dir(&inner, e.parent, self.drive, &mut path_cache) {
                let full = format!("{dir}\\{}", e.name);
                if pred(&full) {
                    out.push((PathBuf::from(full), e.is_dir));
                }
            }
        }
    }

    /// Tail the USN journal, applying creates/deletes/renames to the index.
    fn spawn_usn_tail(self: Arc<Self>) {
        std::thread::Builder::new()
            .name(format!("sc-usn-{}", self.drive))
            .spawn(move || {
                let Ok(vol) = open_volume(self.drive) else { return };
                let mut journal = USN_JOURNAL_DATA_V0::default();
                let mut returned = 0u32;
                let ok = unsafe {
                    DeviceIoControl(
                        vol.0,
                        FSCTL_QUERY_USN_JOURNAL,
                        None,
                        0,
                        Some(&mut journal as *mut _ as *mut _),
                        std::mem::size_of::<USN_JOURNAL_DATA_V0>() as u32,
                        Some(&mut returned),
                        None,
                    )
                };
                if ok.is_err() {
                    return;
                }
                let mut next_usn = journal.NextUsn;
                let mut buf = vec![0u8; 1 << 16];
                while !self.stop.load(Ordering::Relaxed) {
                    let read_data = READ_USN_JOURNAL_DATA_V0 {
                        StartUsn: next_usn,
                        ReasonMask: USN_REASON_FILE_CREATE
                            | USN_REASON_FILE_DELETE
                            | USN_REASON_RENAME_OLD_NAME
                            | USN_REASON_RENAME_NEW_NAME,
                        ReturnOnlyOnClose: 0,
                        Timeout: 1, // seconds; lets us poll the stop flag
                        BytesToWaitFor: 1,
                        UsnJournalID: journal.UsnJournalID,
                    };
                    let mut returned = 0u32;
                    let ok = unsafe {
                        DeviceIoControl(
                            vol.0,
                            FSCTL_READ_USN_JOURNAL,
                            Some(&read_data as *const _ as *const _),
                            std::mem::size_of::<READ_USN_JOURNAL_DATA_V0>() as u32,
                            Some(buf.as_mut_ptr() as *mut _),
                            buf.len() as u32,
                            Some(&mut returned),
                            None,
                        )
                    };
                    if ok.is_err() {
                        std::thread::sleep(std::time::Duration::from_secs(2));
                        continue;
                    }
                    if returned < 8 {
                        continue;
                    }
                    next_usn = i64::from_le_bytes(buf[..8].try_into().unwrap());
                    let mut inner = self.inner.write();
                    unsafe {
                        for_each_record(&buf[8..returned as usize], |rec| {
                            let reason = rec.Reason;
                            if reason & USN_REASON_FILE_DELETE != 0
                                || reason & USN_REASON_RENAME_OLD_NAME != 0
                            {
                                inner.entries.remove(&rec.FileReferenceNumber);
                            }
                            if reason & USN_REASON_FILE_CREATE != 0
                                || reason & USN_REASON_RENAME_NEW_NAME != 0
                            {
                                let name = record_name(rec);
                                inner.entries.insert(
                                    rec.FileReferenceNumber,
                                    IndexEntry {
                                        parent: rec.ParentFileReferenceNumber,
                                        name_lower: name.to_lowercase().into_boxed_str(),
                                        name: name.into_boxed_str(),
                                        is_dir: rec.FileAttributes
                                            & FILE_ATTRIBUTE_DIRECTORY.0
                                            != 0,
                                    },
                                );
                            }
                        });
                    }
                }
            })
            .ok();
    }
}

/// Resolve the full directory path of a parent FRN by walking the chain.
fn resolve_dir(
    inner: &Inner,
    frn: u64,
    drive: char,
    cache: &mut HashMap<u64, Option<String>>,
) -> Option<String> {
    if frn == inner.root_frn {
        return Some(format!("{drive}:"));
    }
    if let Some(cached) = cache.get(&frn) {
        return cached.clone();
    }
    // Walk up iteratively with a depth guard against corrupt chains.
    let mut chain: Vec<u64> = Vec::new();
    let mut cur = frn;
    let mut resolved: Option<String> = None;
    for _ in 0..128 {
        if cur == inner.root_frn {
            resolved = Some(format!("{drive}:"));
            break;
        }
        if let Some(hit) = cache.get(&cur) {
            resolved = hit.clone();
            break;
        }
        match inner.entries.get(&cur) {
            Some(e) => {
                chain.push(cur);
                cur = e.parent;
            }
            None => break,
        }
    }
    let mut path = resolved?;
    for &f in chain.iter().rev() {
        let e = inner.entries.get(&f)?;
        path.push('\\');
        path.push_str(&e.name);
        cache.insert(f, Some(path.clone()));
    }
    Some(path)
}
