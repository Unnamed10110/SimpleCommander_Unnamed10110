//! Shell icon extraction. Icons are fetched by extension (no disk access,
//! via `SHGFI_USEFILEATTRIBUTES`) except for .exe/.lnk/.ico which get
//! per-file icons. HICONs are converted to RGBA for GPU upload.
//!
//! SHGetFileInfo is run on a dedicated STA thread because Office (docx/xlsx)
//! and other icon handlers fail or return blank glyphs from MTA workers.
//! Conversion uses DrawIconEx so 32-bit PNG icons keep their alpha.

use std::ffi::c_void;
use std::os::windows::ffi::OsStrExt;
use std::path::Path;
use std::sync::mpsc::{self, Sender};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;
use windows::core::{PCWSTR, PWSTR};
use windows::Win32::Graphics::Gdi::{
    CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, SelectObject, BITMAPINFO,
    BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, HGDIOBJ,
};
use windows::Win32::Storage::FileSystem::{
    FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_NORMAL, FILE_FLAGS_AND_ATTRIBUTES,
};
use windows::Win32::System::Environment::ExpandEnvironmentStringsW;
use windows::Win32::System::Ole::OleInitialize;
use windows::Win32::UI::Shell::{
    AssocQueryStringW, ExtractIconExW, SHGetFileInfoW, ASSOCF_INIT_DEFAULTTOSTAR, ASSOCF_NOTRUNCATE,
    ASSOCSTR_DEFAULTICON, SHFILEINFOW, SHGFI_ICON, SHGFI_SMALLICON, SHGFI_USEFILEATTRIBUTES,
};
use windows::Win32::UI::WindowsAndMessaging::{
    DestroyIcon, DrawIconEx, GetSystemMetrics, DI_NORMAL, HICON, SM_CXSMICON, SM_CYSMICON,
};

#[derive(Clone)]
pub struct IconRgba {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

struct IconWork {
    dummy: Vec<u16>,
    attrs: FILE_FLAGS_AND_ATTRIBUTES,
    use_attributes: bool,
    reply: Sender<Option<IconRgba>>,
}

fn to_wide(s: &std::ffi::OsStr) -> Vec<u16> {
    s.encode_wide().chain(std::iter::once(0)).collect()
}

/// Extensions that must be resolved per-file (their icon varies per file).
pub fn needs_per_file_icon(ext: &str) -> bool {
    ext.eq_ignore_ascii_case("exe")
        || ext.eq_ignore_ascii_case("lnk")
        || ext.eq_ignore_ascii_case("ico")
        || ext.eq_ignore_ascii_case("cur")
        || ext.eq_ignore_ascii_case("url")
        || ext.eq_ignore_ascii_case("appref-ms")
}

/// Fetch the small shell icon for a file extension without touching disk.
pub fn icon_for_extension(ext: &str, is_dir: bool) -> Option<IconRgba> {
    if is_dir {
        return fetch_icon_sta("folder", FILE_ATTRIBUTE_DIRECTORY, true);
    }
    let ext = ext.trim_start_matches('.');
    if ext.is_empty() {
        return fetch_icon_sta("file", FILE_ATTRIBUTE_NORMAL, true);
    }
    fetch_icon_sta(&format!("file.{ext}"), FILE_ATTRIBUTE_NORMAL, true)
        .or_else(|| fetch_icon_sta(&format!(".{ext}"), FILE_ATTRIBUTE_NORMAL, true))
        .or_else(|| icon_from_assoc(ext))
}

/// Fetch the small shell icon for a concrete path (used for .exe/.lnk).
pub fn icon_for_path(path: &Path) -> Option<IconRgba> {
    let wide = to_wide(path.as_os_str());
    sta_shgfi(wide, FILE_ATTRIBUTE_NORMAL, false)
}

fn fetch_icon_sta(dummy: &str, attrs: FILE_FLAGS_AND_ATTRIBUTES, use_attributes: bool) -> Option<IconRgba> {
    sta_shgfi(to_wide(std::ffi::OsStr::new(dummy)), attrs, use_attributes)
}

fn sta_tx() -> Sender<IconWork> {
    static TX: OnceLock<Mutex<Sender<IconWork>>> = OnceLock::new();
    TX.get_or_init(|| {
        let (tx, rx) = mpsc::channel::<IconWork>();
        let _ = std::thread::Builder::new()
            .name("sc-icons".into())
            .spawn(move || {
                unsafe {
                    let _ = OleInitialize(None);
                }
                while let Ok(work) = rx.recv() {
                    let rgba = shgfi_to_rgba(&work.dummy, work.attrs, work.use_attributes);
                    let _ = work.reply.send(rgba);
                }
            });
        Mutex::new(tx)
    })
    .lock()
    .expect("icon sta mutex")
    .clone()
}

fn sta_shgfi(
    dummy: Vec<u16>,
    attrs: FILE_FLAGS_AND_ATTRIBUTES,
    use_attributes: bool,
) -> Option<IconRgba> {
    let (rtx, rrx) = mpsc::channel();
    sta_tx()
        .send(IconWork {
            dummy,
            attrs,
            use_attributes,
            reply: rtx,
        })
        .ok()?;
    rrx.recv_timeout(Duration::from_millis(800)).ok().flatten()
}

fn shgfi_to_rgba(
    wide_path: &[u16],
    attrs: FILE_FLAGS_AND_ATTRIBUTES,
    use_attributes: bool,
) -> Option<IconRgba> {
    let mut info = SHFILEINFOW::default();
    let mut flags = SHGFI_ICON | SHGFI_SMALLICON;
    if use_attributes {
        flags |= SHGFI_USEFILEATTRIBUTES;
    }
    let res = unsafe {
        SHGetFileInfoW(
            PCWSTR::from_raw(wide_path.as_ptr()),
            attrs,
            Some(&mut info),
            std::mem::size_of::<SHFILEINFOW>() as u32,
            flags,
        )
    };
    if res == 0 || info.hIcon.is_invalid() {
        return None;
    }
    let rgba = hicon_to_rgba(info.hIcon);
    unsafe {
        let _ = DestroyIcon(info.hIcon);
    }
    rgba
}

fn icon_from_assoc(ext: &str) -> Option<IconRgba> {
    let assoc = to_wide(std::ffi::OsStr::new(&format!(".{ext}")));
    let spec = assoc_default_icon(&assoc)?;
    let (path, index) = parse_default_icon(&spec)?;
    extract_icon_file(&path, index)
}

fn assoc_default_icon(assoc: &[u16]) -> Option<String> {
    unsafe {
        let mut n = 0u32;
        let flags = ASSOCF_NOTRUNCATE | ASSOCF_INIT_DEFAULTTOSTAR;
        let _ = AssocQueryStringW(
            flags,
            ASSOCSTR_DEFAULTICON,
            PCWSTR::from_raw(assoc.as_ptr()),
            PCWSTR::null(),
            None,
            &mut n,
        );
        if n == 0 {
            return None;
        }
        let mut buf = vec![0u16; n as usize];
        let hr = AssocQueryStringW(
            flags,
            ASSOCSTR_DEFAULTICON,
            PCWSTR::from_raw(assoc.as_ptr()),
            PCWSTR::null(),
            Some(PWSTR(buf.as_mut_ptr())),
            &mut n,
        );
        if hr.is_err() {
            return None;
        }
        let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        let s = String::from_utf16_lossy(&buf[..end]);
        if s.is_empty() {
            None
        } else {
            Some(s)
        }
    }
}

fn parse_default_icon(spec: &str) -> Option<(String, i32)> {
    let spec = spec.trim();
    if spec.is_empty() || spec == "%1" || spec.eq_ignore_ascii_case("\"%1\"") {
        return None;
    }
    let (path, index) = match spec.rfind(',') {
        Some(i) => {
            let idx = spec[i + 1..].trim().parse::<i32>().ok()?;
            (spec[..i].trim(), idx)
        }
        None => (spec, 0),
    };
    let path = path.trim_matches('"').trim();
    if path.is_empty() {
        return None;
    }
    Some((expand_env(path), index))
}

fn expand_env(s: &str) -> String {
    let wide = to_wide(std::ffi::OsStr::new(s));
    let mut buf = vec![0u16; wide.len() + 260];
    let n = unsafe { ExpandEnvironmentStringsW(PCWSTR::from_raw(wide.as_ptr()), Some(&mut buf)) };
    if n == 0 {
        return s.to_string();
    }
    let n = (n as usize).saturating_sub(1).min(buf.len());
    String::from_utf16_lossy(&buf[..n])
}

fn extract_icon_file(path: &str, index: i32) -> Option<IconRgba> {
    let wide = to_wide(std::ffi::OsStr::new(path));
    let mut small = HICON::default();
    let mut large = HICON::default();
    let n = unsafe {
        ExtractIconExW(
            PCWSTR::from_raw(wide.as_ptr()),
            index,
            Some(&mut large),
            Some(&mut small),
            1,
        )
    };
    if n == 0 {
        return None;
    }
    let hicon = if !small.is_invalid() {
        if !large.is_invalid() {
            unsafe {
                let _ = DestroyIcon(large);
            }
        }
        small
    } else {
        large
    };
    if hicon.is_invalid() {
        return None;
    }
    let rgba = hicon_to_rgba(hicon);
    unsafe {
        let _ = DestroyIcon(hicon);
    }
    rgba
}

/// Convert an HICON into RGBA pixels by drawing it onto a 32-bit DIB.
pub fn hicon_to_rgba(hicon: HICON) -> Option<IconRgba> {
    unsafe {
        let w = GetSystemMetrics(SM_CXSMICON).max(16) as u32;
        let h = GetSystemMetrics(SM_CYSMICON).max(16) as u32;
        let hdc = CreateCompatibleDC(None);
        if hdc.is_invalid() {
            return None;
        }
        let header = BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: w as i32,
            biHeight: -(h as i32),
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        };
        let bi = BITMAPINFO {
            bmiHeader: header,
            ..Default::default()
        };
        let mut bits: *mut c_void = std::ptr::null_mut();
        let hbmp = match CreateDIBSection(Some(hdc), &bi, DIB_RGB_COLORS, &mut bits, None, 0) {
            Ok(b) if !b.is_invalid() => b,
            _ => {
                let _ = DeleteDC(hdc);
                return None;
            }
        };
        let old = SelectObject(hdc, HGDIOBJ(hbmp.0));
        let drawn = DrawIconEx(hdc, 0, 0, hicon, w as i32, h as i32, 0, None, DI_NORMAL).is_ok();
        let rgba = if drawn && !bits.is_null() {
            let len = (w * h * 4) as usize;
            let src = std::slice::from_raw_parts(bits as *const u8, len);
            let mut pixels = src.to_vec();
            for px in pixels.chunks_exact_mut(4) {
                px.swap(0, 2);
            }
            Some(IconRgba {
                width: w,
                height: h,
                rgba: pixels,
            })
        } else {
            None
        };
        SelectObject(hdc, old);
        let _ = DeleteObject(HGDIOBJ(hbmp.0));
        let _ = DeleteDC(hdc);
        rgba
    }
}
