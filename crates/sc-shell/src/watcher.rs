//! Live directory watching via `ReadDirectoryChangesW` (overlapped, with a
//! stop event). Changes are debounced and delivered as a coarse "directory
//! changed" signal; the UI re-enumerates, which is simpler and safer than
//! patching diffs into snapshots.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use windows::core::PCWSTR;
use windows::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, ReadDirectoryChangesW, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OVERLAPPED,
    FILE_LIST_DIRECTORY, FILE_NOTIFY_CHANGE_ATTRIBUTES, FILE_NOTIFY_CHANGE_DIR_NAME,
    FILE_NOTIFY_CHANGE_FILE_NAME, FILE_NOTIFY_CHANGE_LAST_WRITE, FILE_NOTIFY_CHANGE_SIZE,
    FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
};
use windows::Win32::System::Threading::{CreateEventW, SetEvent, WaitForMultipleObjects, INFINITE};
use windows::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};

use std::os::windows::ffi::OsStrExt;

/// Handle to a running watcher thread. Dropping stops the watcher.
pub struct DirWatcher {
    stop: Arc<AtomicBool>,
    stop_event: isize,
    thread: Option<std::thread::JoinHandle<()>>,
    path: PathBuf,
    subtree: bool,
}

// HANDLE values are just kernel object references; safe to move across threads.
unsafe impl Send for DirWatcher {}

impl DirWatcher {
    /// Watch `path`; calls `on_change(watch_id)` (debounced ~200 ms) whenever
    /// anything inside changes. `subtree` should only be true for flatten-branch
    /// view — watching a whole tree (e.g. `target/`) re-lists the pane constantly.
    pub fn spawn(
        path: &Path,
        watch_id: u64,
        subtree: bool,
        on_change: impl Fn(u64) + Send + 'static,
    ) -> Option<Self> {
        let wide: Vec<u16> = path
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let dir_handle = unsafe {
            CreateFileW(
                PCWSTR::from_raw(wide.as_ptr()),
                FILE_LIST_DIRECTORY.0,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                None,
                OPEN_EXISTING,
                FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OVERLAPPED,
                None,
            )
        }
        .ok()?;

        let stop = Arc::new(AtomicBool::new(false));
        let stop_clone = stop.clone();
        let stop_event = unsafe { CreateEventW(None, true, false, None) }.ok()?;
        let io_event = unsafe { CreateEventW(None, true, false, None) }.ok()?;
        let stop_event_raw = stop_event.0 as isize;
        let dir_raw = dir_handle.0 as isize;
        let io_raw = io_event.0 as isize;

        let thread = std::thread::Builder::new()
            .name("sc-watcher".into())
            .spawn(move || {
                let dir = HANDLE(dir_raw as _);
                let io_ev = HANDLE(io_raw as _);
                let stop_ev = HANDLE(stop_event_raw as _);
                let mut buf = vec![0u8; 64 * 1024];
                'outer: while !stop_clone.load(Ordering::Relaxed) {
                    let mut overlapped = OVERLAPPED::default();
                    overlapped.hEvent = io_ev;
                    let ok = unsafe {
                        ReadDirectoryChangesW(
                            dir,
                            buf.as_mut_ptr() as *mut _,
                            buf.len() as u32,
                            subtree,
                            FILE_NOTIFY_CHANGE_FILE_NAME
                                | FILE_NOTIFY_CHANGE_DIR_NAME
                                | FILE_NOTIFY_CHANGE_ATTRIBUTES
                                | FILE_NOTIFY_CHANGE_SIZE
                                | FILE_NOTIFY_CHANGE_LAST_WRITE,
                            None,
                            Some(&mut overlapped),
                            None,
                        )
                    };
                    if ok.is_err() {
                        break;
                    }
                    let handles = [stop_ev, io_ev];
                    let wait = unsafe { WaitForMultipleObjects(&handles, false, INFINITE) };
                    if wait == WAIT_OBJECT_0 {
                        // Stop requested.
                        unsafe {
                            let _ = CancelIoEx(dir, Some(&overlapped));
                        }
                        break 'outer;
                    }
                    let mut transferred = 0u32;
                    let res =
                        unsafe { GetOverlappedResult(dir, &overlapped, &mut transferred, true) };
                    if res.is_err() {
                        break;
                    }
                    // Debounce: absorb follow-up changes for a short window.
                    std::thread::sleep(Duration::from_millis(200));
                    if stop_clone.load(Ordering::Relaxed) {
                        break;
                    }
                    on_change(watch_id);
                }
                unsafe {
                    let _ = CloseHandle(dir);
                    let _ = CloseHandle(io_ev);
                }
            })
            .ok()?;

        Some(Self {
            stop,
            stop_event: stop_event_raw,
            thread: Some(thread),
            path: path.to_path_buf(),
            subtree,
        })
    }

    pub fn watches(&self, path: &Path, subtree: bool) -> bool {
        self.path == path && self.subtree == subtree
    }
}

impl Drop for DirWatcher {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        unsafe {
            let _ = SetEvent(HANDLE(self.stop_event as _));
        }
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
        unsafe {
            let _ = CloseHandle(HANDLE(self.stop_event as _));
        }
    }
}
