//! Shell icon extraction. Icons are fetched by extension (no disk access,
//! via `SHGFI_USEFILEATTRIBUTES`) except for .exe/.lnk/.ico which get
//! per-file icons. HICONs are converted to RGBA for GPU upload.

use std::path::Path;
use windows::core::PCWSTR;
use windows::Win32::Graphics::Gdi::{
    DeleteObject, GetDC, GetDIBits, ReleaseDC, BITMAPINFO, BITMAPINFOHEADER, BI_RGB,
    DIB_RGB_COLORS, HGDIOBJ,
};
use windows::Win32::Storage::FileSystem::{
    FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_NORMAL, FILE_FLAGS_AND_ATTRIBUTES,
};
use windows::Win32::UI::Shell::{
    SHGetFileInfoW, SHFILEINFOW, SHGFI_ICON, SHGFI_SMALLICON, SHGFI_USEFILEATTRIBUTES,
};
use windows::Win32::UI::WindowsAndMessaging::{DestroyIcon, GetIconInfo, HICON, ICONINFO};

use std::os::windows::ffi::OsStrExt;

#[derive(Clone)]
pub struct IconRgba {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
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
    let dummy = if is_dir {
        "folder".to_string()
    } else if ext.is_empty() {
        "file".to_string()
    } else {
        format!("file.{ext}")
    };
    let wide = to_wide(std::ffi::OsStr::new(&dummy));
    let attrs = if is_dir { FILE_ATTRIBUTE_DIRECTORY } else { FILE_ATTRIBUTE_NORMAL };
    fetch_icon(&wide, attrs, true)
}

/// Fetch the small shell icon for a concrete path (used for .exe/.lnk).
pub fn icon_for_path(path: &Path) -> Option<IconRgba> {
    let wide = to_wide(path.as_os_str());
    fetch_icon(&wide, FILE_ATTRIBUTE_NORMAL, false)
}

fn fetch_icon(
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

/// Convert an HICON into RGBA pixels using GDI.
pub fn hicon_to_rgba(hicon: HICON) -> Option<IconRgba> {
    unsafe {
        let mut ii = ICONINFO::default();
        GetIconInfo(hicon, &mut ii).ok()?;
        // Ensure cleanup of the two bitmaps GetIconInfo allocates.
        let color = ii.hbmColor;
        let mask = ii.hbmMask;
        let result = (|| {
            if color.is_invalid() {
                return None; // monochrome icon; not worth supporting
            }
            let hdc = GetDC(None);
            if hdc.is_invalid() {
                return None;
            }
            let mut header = BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                ..Default::default()
            };
            let mut bi = BITMAPINFO { bmiHeader: header, ..Default::default() };
            // First call: query dimensions.
            if GetDIBits(hdc, color, 0, 0, None, &mut bi, DIB_RGB_COLORS) == 0 {
                ReleaseDC(None, hdc);
                return None;
            }
            let w = bi.bmiHeader.biWidth.unsigned_abs();
            let h = bi.bmiHeader.biHeight.unsigned_abs();
            if w == 0 || h == 0 || w > 512 || h > 512 {
                ReleaseDC(None, hdc);
                return None;
            }
            header.biWidth = w as i32;
            header.biHeight = -(h as i32); // top-down
            header.biPlanes = 1;
            header.biBitCount = 32;
            header.biCompression = BI_RGB.0;
            let mut bi = BITMAPINFO { bmiHeader: header, ..Default::default() };
            let mut pixels = vec![0u8; (w * h * 4) as usize];
            let got = GetDIBits(
                hdc,
                color,
                0,
                h,
                Some(pixels.as_mut_ptr() as *mut _),
                &mut bi,
                DIB_RGB_COLORS,
            );
            if got == 0 {
                ReleaseDC(None, hdc);
                return None;
            }
            // BGRA -> RGBA.
            let has_alpha = pixels.chunks_exact(4).any(|p| p[3] != 0);
            for px in pixels.chunks_exact_mut(4) {
                px.swap(0, 2);
            }
            if !has_alpha {
                // Derive alpha from the AND mask.
                let mut mask_px = vec![0u8; (w * h * 4) as usize];
                let mut mbi = BITMAPINFO { bmiHeader: header, ..Default::default() };
                if GetDIBits(
                    hdc,
                    mask,
                    0,
                    h,
                    Some(mask_px.as_mut_ptr() as *mut _),
                    &mut mbi,
                    DIB_RGB_COLORS,
                ) != 0
                {
                    for (px, m) in pixels.chunks_exact_mut(4).zip(mask_px.chunks_exact(4)) {
                        px[3] = if m[0] == 0 { 255 } else { 0 };
                    }
                } else {
                    for px in pixels.chunks_exact_mut(4) {
                        px[3] = 255;
                    }
                }
            }
            ReleaseDC(None, hdc);
            Some(IconRgba { width: w, height: h, rgba: pixels })
        })();
        let _ = DeleteObject(HGDIOBJ(color.0));
        let _ = DeleteObject(HGDIOBJ(mask.0));
        result
    }
}
