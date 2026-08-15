//! Query voidtools Everything over WM_COPYDATA IPC when it is installed.
//! Falls back to `None` so callers can use the built-in MFT/walk index.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Sender};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows_core::BOOL;
use windows::Win32::System::DataExchange::COPYDATASTRUCT;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Registry::{
    RegGetValueW, HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, REG_VALUE_TYPE, RRF_RT_REG_SZ,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, EnumWindows,
    GetClassNameW, GetWindowLongPtrW, PeekMessageW, RegisterClassExW, SendMessageTimeoutW,
    SetWindowLongPtrW, TranslateMessage, CS_DBLCLKS, CW_USEDEFAULT, GWLP_USERDATA, PM_REMOVE,
    SMTO_ABORTIFHUNG, SMTO_BLOCK, WINDOW_EX_STYLE, WM_COPYDATA, WNDCLASSEXW,
    WS_POPUP, MSG,
};

const COPYDATA_QUERY_W: usize = 2;
const COPYDATA_QUERY_COMPLETE: u32 = 0;
const IPC_FOLDER: u32 = 1;
const OUR_CLASS: PCWSTR = w!("SimpleCommander.EverythingIpc");

#[repr(C)]
struct QueryHeader {
    reply_hwnd: u32,
    reply_copydata_message: u32,
    search_flags: u32,
    offset: u32,
    max_results: u32,
}

#[repr(C)]
struct ListHeader {
    totfolders: u32,
    totfiles: u32,
    totitems: u32,
    numfolders: u32,
    numfiles: u32,
    numitems: u32,
    offset: u32,
}

#[repr(C)]
struct Item {
    flags: u32,
    filename_offset: u32,
    path_offset: u32,
}

struct ReplyState {
    items: Vec<(PathBuf, bool)>,
    got: bool,
}

struct Work {
    query: String,
    max: u32,
    reply: Sender<Option<Vec<(PathBuf, bool)>>>,
}

static TX: OnceLock<Mutex<Sender<Work>>> = OnceLock::new();

/// Start Everything in the tray if it is installed but not running.
pub fn warmup() {
    ensure_running();
}

/// True when Everything's IPC window is present (it is running).
pub fn is_running() -> bool {
    ipc_hwnd().is_some()
}

/// True when an Everything.exe install was found on disk.
pub fn is_installed() -> bool {
    find_everything_exe().is_some()
}

pub const DOWNLOAD_PAGE: &str = "https://www.voidtools.com/forum/viewtopic.php?t=17663";

/// Open the Everything download / install page in the default browser.
pub fn open_download_page() {
    let _ = std::process::Command::new("explorer").arg(DOWNLOAD_PAGE).spawn();
}

/// Search the Everything index. `None` means Everything is not available.
pub fn search(query: &str, max: usize) -> Option<Vec<(PathBuf, bool)>> {
    let query = query.trim();
    if query.is_empty() {
        return Some(Vec::new());
    }
    ensure_running();
    if !is_running() {
        return None;
    }
    let tx = ipc_thread();
    let (rtx, rrx) = mpsc::channel();
    tx.lock()
        .unwrap()
        .send(Work {
            query: query.to_string(),
            max: max.max(1) as u32,
            reply: rtx,
        })
        .ok()?;
    rrx.recv_timeout(Duration::from_secs(4)).ok().flatten()
}

/// Restrict an Everything query to a folder (recursive).
pub fn search_in(dir: &Path, query: &str, max: usize) -> Option<Vec<(PathBuf, bool)>> {
    let dir = dir.to_string_lossy().replace('"', "");
    let dir = dir.trim_end_matches(['\\', '/']);
    if dir.is_empty() {
        return search(query, max);
    }
    let q = format!("path:\"{dir}\" {query}");
    search(&q, max)
}

fn ipc_hwnd() -> Option<HWND> {
    let mut found = HWND::default();
    unsafe {
        let _ = EnumWindows(Some(enum_ipc_windows), LPARAM(&mut found as *mut HWND as isize));
    }
    if found.is_invalid() {
        None
    } else {
        Some(found)
    }
}

unsafe extern "system" fn enum_ipc_windows(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let mut buf = [0u16; 256];
    let n = unsafe { GetClassNameW(hwnd, &mut buf) };
    if n <= 0 {
        return BOOL(1);
    }
    let class = String::from_utf16_lossy(&buf[..n as usize]);
    // 1.4: EVERYTHING_TASKBAR_NOTIFICATION
    // 1.5a: EVERYTHING_TASKBAR_NOTIFICATION_(1.5a)
    if class == "EVERYTHING_TASKBAR_NOTIFICATION"
        || class.starts_with("EVERYTHING_TASKBAR_NOTIFICATION_")
    {
        unsafe { *(lparam.0 as *mut HWND) = hwnd };
        return BOOL(0);
    }
    BOOL(1)
}

fn ipc_thread() -> &'static Mutex<Sender<Work>> {
    TX.get_or_init(|| {
        let (tx, rx) = mpsc::channel::<Work>();
        std::thread::Builder::new()
            .name("sc-everything-ipc".into())
            .spawn(move || {
                register_class();
                while let Ok(work) = rx.recv() {
                    let hits = perform_query(&work.query, work.max);
                    let _ = work.reply.send(hits);
                }
            })
            .ok();
        Mutex::new(tx)
    })
}

fn ensure_running() {
    if is_running() {
        return;
    }
    static START: OnceLock<()> = OnceLock::new();
    START.get_or_init(|| {
        let Some(exe) = find_everything_exe() else {
            return;
        };
        let _ = std::process::Command::new(exe).arg("-startup").spawn();
        let deadline = Instant::now() + Duration::from_secs(3);
        while Instant::now() < deadline {
            if is_running() {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    });
}

fn find_everything_exe() -> Option<PathBuf> {
    const PATHS: &[&str] = &[
        r"C:\Program Files\Everything\Everything.exe",
        r"C:\Program Files (x86)\Everything\Everything.exe",
        r"C:\Program Files\Everything 1.5a\Everything64.exe",
        r"C:\Program Files\Everything 1.5a\Everything.exe",
        r"C:\Program Files\voidtools\Everything\Everything.exe",
    ];
    for p in PATHS {
        let p = PathBuf::from(p);
        if p.is_file() {
            return Some(p);
        }
    }
    const KEYS: &[(HKEY, PCWSTR)] = &[
        (HKEY_CURRENT_USER, w!("Software\\voidtools\\Everything")),
        (HKEY_LOCAL_MACHINE, w!("SOFTWARE\\voidtools\\Everything")),
        (
            HKEY_LOCAL_MACHINE,
            w!("SOFTWARE\\WOW6432Node\\voidtools\\Everything"),
        ),
        (HKEY_CURRENT_USER, w!("Software\\voidtools\\Everything 1.5a")),
        (HKEY_LOCAL_MACHINE, w!("SOFTWARE\\voidtools\\Everything 1.5a")),
        (
            HKEY_LOCAL_MACHINE,
            w!("SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\App Paths\\Everything.exe"),
        ),
    ];
    for (root, key) in KEYS {
        for value in [w!(""), w!("InstallLocation"), w!("Path"), w!("exe_path")] {
            if let Some(s) = reg_sz(*root, *key, value) {
                let p = PathBuf::from(s.trim_matches('"'));
                if p.is_file() {
                    return Some(p);
                }
                let exe = p.join("Everything.exe");
                if exe.is_file() {
                    return Some(exe);
                }
                let exe64 = p.join("Everything64.exe");
                if exe64.is_file() {
                    return Some(exe64);
                }
            }
        }
    }
    None
}

fn reg_sz(root: HKEY, key: PCWSTR, value: PCWSTR) -> Option<String> {
    unsafe {
        let mut buf = [0u16; 520];
        let mut size = (buf.len() * 2) as u32;
        let mut kind = REG_VALUE_TYPE::default();
        if RegGetValueW(
            root,
            key,
            value,
            RRF_RT_REG_SZ,
            Some(&mut kind),
            Some(buf.as_mut_ptr() as *mut _),
            Some(&mut size),
        )
        .is_err()
        {
            return None;
        }
        let nchars = (size as usize / 2).saturating_sub(1).min(buf.len());
        let s = String::from_utf16_lossy(&buf[..nchars]);
        if s.is_empty() {
            None
        } else {
            Some(s)
        }
    }
}

fn register_class() {
    unsafe {
        let hinstance = GetModuleHandleW(None).ok().map(|m| m.into());
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: CS_DBLCLKS,
            lpfnWndProc: Some(wndproc),
            hInstance: hinstance.unwrap_or_default(),
            lpszClassName: OUR_CLASS,
            ..Default::default()
        };
        let _ = RegisterClassExW(&wc);
    }
}

fn perform_query(query: &str, max: u32) -> Option<Vec<(PathBuf, bool)>> {
    let ev = ipc_hwnd()?;
    unsafe {
        // A real top-level window: HWND_MESSAGE often never receives WM_COPYDATA
        // from another process, which is how Everything delivers results.
        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            OUR_CLASS,
            w!("sc-ev"),
            WS_POPUP,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            0,
            0,
            None,
            None,
            None,
            None,
        )
        .ok()?;
        let mut state = ReplyState {
            items: Vec::new(),
            got: false,
        };
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, &mut state as *mut _ as isize);

        let wide: Vec<u16> = query.encode_utf16().chain(std::iter::once(0)).collect();
        let header = QueryHeader {
            reply_hwnd: hwnd.0 as usize as u32,
            reply_copydata_message: COPYDATA_QUERY_COMPLETE,
            search_flags: 0,
            offset: 0,
            max_results: max,
        };
        let mut buf = vec![0u8; std::mem::size_of::<QueryHeader>() + wide.len() * 2];
        buf[..std::mem::size_of::<QueryHeader>()].copy_from_slice(std::slice::from_raw_parts(
            (&header as *const QueryHeader) as *const u8,
            std::mem::size_of::<QueryHeader>(),
        ));
        let bytes = std::slice::from_raw_parts(wide.as_ptr() as *const u8, wide.len() * 2);
        buf[std::mem::size_of::<QueryHeader>()..].copy_from_slice(bytes);

        let cds = COPYDATASTRUCT {
            dwData: COPYDATA_QUERY_W,
            cbData: buf.len() as u32,
            lpData: buf.as_mut_ptr() as *mut _,
        };
        let mut result = 0usize;
        let _ = SendMessageTimeoutW(
            ev,
            WM_COPYDATA,
            WPARAM(hwnd.0 as usize),
            LPARAM(&cds as *const COPYDATASTRUCT as isize),
            SMTO_BLOCK | SMTO_ABORTIFHUNG,
            3000,
            Some(&mut result),
        );

        // Everything often *posts* the result list after SendMessage returns.
        let deadline = Instant::now() + Duration::from_millis(2000);
        while !state.got && Instant::now() < deadline {
            let mut msg = MSG::default();
            while PeekMessageW(&mut msg, Some(hwnd), 0, 0, PM_REMOVE).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
            if !state.got {
                std::thread::sleep(Duration::from_millis(5));
            }
        }

        let got = state.got;
        let items = std::mem::take(&mut state.items);
        let _ = DestroyWindow(hwnd);
        if !got {
            return None;
        }
        Some(items)
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if msg == WM_COPYDATA {
        let cds = &*(lparam.0 as *const COPYDATASTRUCT);
        if cds.dwData == COPYDATA_QUERY_COMPLETE as usize && !cds.lpData.is_null() {
            let state = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut ReplyState;
            if !state.is_null() {
                parse_list(cds.lpData as *const u8, cds.cbData as usize, &mut (*state).items);
                (*state).got = true;
            }
            return LRESULT(1);
        }
    }
    DefWindowProcW(hwnd, msg, wparam, lparam)
}

fn parse_list(base: *const u8, len: usize, out: &mut Vec<(PathBuf, bool)>) {
    if len < std::mem::size_of::<ListHeader>() {
        return;
    }
    let header = unsafe { &*(base as *const ListHeader) };
    let n = header.numitems as usize;
    let items_off = std::mem::size_of::<ListHeader>();
    let items_bytes = n.saturating_mul(std::mem::size_of::<Item>());
    if items_off + items_bytes > len {
        return;
    }
    out.reserve(n);
    for i in 0..n {
        let item = unsafe { &*((base.add(items_off + i * std::mem::size_of::<Item>())) as *const Item) };
        let name = match read_wcs(base, len, item.filename_offset) {
            Some(s) => s,
            None => continue,
        };
        let path = read_wcs(base, len, item.path_offset).unwrap_or_default();
        let full = join_ev(&path, &name);
        let is_dir = item.flags & IPC_FOLDER != 0;
        out.push((full, is_dir));
    }
}

fn read_wcs(base: *const u8, len: usize, offset: u32) -> Option<String> {
    let off = offset as usize;
    if off >= len || off % 2 != 0 {
        return None;
    }
    let max_chars = (len - off) / 2;
    unsafe {
        let p = base.add(off) as *const u16;
        let mut n = 0usize;
        while n < max_chars && *p.add(n) != 0 {
            n += 1;
        }
        if n == 0 {
            return Some(String::new());
        }
        Some(String::from_utf16_lossy(std::slice::from_raw_parts(p, n)))
    }
}

fn join_ev(path: &str, name: &str) -> PathBuf {
    if path.is_empty() {
        PathBuf::from(name)
    } else if path.ends_with('\\') || path.ends_with('/') {
        PathBuf::from(format!("{path}{name}"))
    } else {
        PathBuf::from(format!("{path}\\{name}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ipc_returns_hits_when_everything_is_running() {
        warmup();
        if !is_running() {
            eprintln!("skip: Everything IPC window not found");
            return;
        }
        let hits = search("*.txt", 20).expect("Everything IPC query failed");
        assert!(
            !hits.is_empty(),
            "Everything is running but returned 0 hits for *.txt"
        );
    }
}
