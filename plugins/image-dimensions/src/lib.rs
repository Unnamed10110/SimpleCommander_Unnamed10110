//! SimpleCommander column plugin: shows image dimensions (PNG/JPEG/GIF/BMP).
//!
//! ABI: see sc-plugins/src/host.rs. Exports sc_alloc / sc_manifest /
//! sc_column_value; imports sc.read_file (requires the "read-files" grant).

use std::alloc::{alloc, Layout};

#[link(wasm_import_module = "sc")]
extern "C" {
    fn read_file(ptr: i32, len: i32) -> i64;
}

#[no_mangle]
pub extern "C" fn sc_alloc(len: i32) -> i32 {
    if len <= 0 {
        return 0;
    }
    unsafe { alloc(Layout::from_size_align_unchecked(len as usize, 1)) as i32 }
}

fn pack(data: Vec<u8>) -> i64 {
    let len = data.len() as i64 & 0xFFFF_FFFF;
    let ptr = data.leak().as_ptr() as i64;
    (ptr << 32) | len
}

const MANIFEST: &str = r#"{
  "name": "Image Dimensions",
  "version": "0.1.0",
  "description": "Shows pixel dimensions of images in a custom column.",
  "kinds": ["column"],
  "extensions": ["png", "jpg", "jpeg", "gif", "bmp"],
  "permissions": ["read-files"],
  "column_title": "Dimensions"
}"#;

#[no_mangle]
pub extern "C" fn sc_manifest() -> i64 {
    pack(MANIFEST.as_bytes().to_vec())
}

#[no_mangle]
pub extern "C" fn sc_column_value(path_ptr: i32, path_len: i32) -> i64 {
    let packed = unsafe { read_file(path_ptr, path_len) };
    if packed == 0 {
        return 0;
    }
    let ptr = ((packed >> 32) & 0xFFFF_FFFF) as u32 as *const u8;
    let len = (packed & 0xFFFF_FFFF) as usize;
    let data = unsafe { std::slice::from_raw_parts(ptr, len) };
    match dimensions(data) {
        Some((w, h)) => pack(format!("{w} x {h}").into_bytes()),
        None => 0,
    }
}

fn dimensions(d: &[u8]) -> Option<(u32, u32)> {
    if d.len() >= 24 && d.starts_with(&[0x89, b'P', b'N', b'G']) {
        let w = u32::from_be_bytes([d[16], d[17], d[18], d[19]]);
        let h = u32::from_be_bytes([d[20], d[21], d[22], d[23]]);
        return Some((w, h));
    }
    if d.len() >= 10 && (d.starts_with(b"GIF87a") || d.starts_with(b"GIF89a")) {
        let w = u16::from_le_bytes([d[6], d[7]]) as u32;
        let h = u16::from_le_bytes([d[8], d[9]]) as u32;
        return Some((w, h));
    }
    if d.len() >= 26 && d.starts_with(b"BM") {
        let w = i32::from_le_bytes([d[18], d[19], d[20], d[21]]).unsigned_abs();
        let h = i32::from_le_bytes([d[22], d[23], d[24], d[25]]).unsigned_abs();
        return Some((w, h));
    }
    if d.len() >= 4 && d[0] == 0xFF && d[1] == 0xD8 {
        return jpeg_dimensions(d);
    }
    None
}

fn jpeg_dimensions(d: &[u8]) -> Option<(u32, u32)> {
    let mut i = 2usize;
    while i + 9 < d.len() {
        if d[i] != 0xFF {
            i += 1;
            continue;
        }
        let marker = d[i + 1];
        // SOF0..SOF15 except DHT(C4), JPG(C8), DAC(CC).
        if (0xC0..=0xCF).contains(&marker) && marker != 0xC4 && marker != 0xC8 && marker != 0xCC {
            let h = u16::from_be_bytes([d[i + 5], d[i + 6]]) as u32;
            let w = u16::from_be_bytes([d[i + 7], d[i + 8]]) as u32;
            return Some((w, h));
        }
        let seg_len = u16::from_be_bytes([d[i + 2], d[i + 3]]) as usize;
        i += 2 + seg_len;
    }
    None
}
