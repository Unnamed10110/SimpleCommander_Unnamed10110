//! SimpleCommander command plugin: CRC32 checksums for the selected files.

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
  "name": "CRC32 Checksums",
  "version": "0.1.0",
  "description": "Computes CRC32 checksums of the selected files.",
  "kinds": ["command"],
  "permissions": ["read-files"],
  "command_label": "CRC32 of selection"
}"#;

#[no_mangle]
pub extern "C" fn sc_manifest() -> i64 {
    pack(MANIFEST.as_bytes().to_vec())
}

#[no_mangle]
pub extern "C" fn sc_run_command(input_ptr: i32, input_len: i32) -> i64 {
    let input = unsafe { std::slice::from_raw_parts(input_ptr as *const u8, input_len as usize) };
    let json = String::from_utf8_lossy(input);
    let paths = parse_json_string_array(&json);
    let mut out = String::new();
    for p in paths {
        let packed = unsafe { read_file(p.as_ptr() as i32, p.len() as i32) };
        if packed == 0 {
            out.push_str(&format!("{p}: <unreadable or permission not granted>\n"));
            continue;
        }
        let ptr = ((packed >> 32) & 0xFFFF_FFFF) as u32 as *const u8;
        let len = (packed & 0xFFFF_FFFF) as usize;
        let data = unsafe { std::slice::from_raw_parts(ptr, len) };
        out.push_str(&format!("{p}: {:08X} ({len} bytes)\n", crc32(data)));
    }
    if out.is_empty() {
        out.push_str("No files selected.");
    }
    pack(out.into_bytes())
}

/// Minimal parser for a JSON array of strings (handles \" \\ \/ \n \t escapes).
fn parse_json_string_array(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'"' {
            let mut cur = String::new();
            i += 1;
            while i < bytes.len() && bytes[i] != b'"' {
                if bytes[i] == b'\\' && i + 1 < bytes.len() {
                    let esc = bytes[i + 1];
                    match esc {
                        b'"' => cur.push('"'),
                        b'\\' => cur.push('\\'),
                        b'/' => cur.push('/'),
                        b'n' => cur.push('\n'),
                        b't' => cur.push('\t'),
                        b'r' => cur.push('\r'),
                        b'u' => {
                            if i + 5 < bytes.len() {
                                if let Ok(code) =
                                    u32::from_str_radix(&s[i + 2..i + 6], 16)
                                {
                                    if let Some(c) = char::from_u32(code) {
                                        cur.push(c);
                                    }
                                }
                                i += 4;
                            }
                        }
                        _ => {}
                    }
                    i += 2;
                } else {
                    // Copy one UTF-8 scalar.
                    let ch_len = utf8_len(bytes[i]);
                    if let Ok(chunk) = std::str::from_utf8(&bytes[i..(i + ch_len).min(bytes.len())])
                    {
                        cur.push_str(chunk);
                    }
                    i += ch_len;
                }
            }
            out.push(cur);
        }
        i += 1;
    }
    out
}

fn utf8_len(b: u8) -> usize {
    match b {
        0x00..=0x7F => 1,
        0xC0..=0xDF => 2,
        0xE0..=0xEF => 3,
        _ => 4,
    }
}

fn crc32(data: &[u8]) -> u32 {
    let mut table = [0u32; 256];
    for i in 0..256u32 {
        let mut c = i;
        for _ in 0..8 {
            c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
        }
        table[i as usize] = c;
    }
    let mut crc = 0xFFFF_FFFFu32;
    for &b in data {
        crc = table[((crc ^ b as u32) & 0xFF) as usize] ^ (crc >> 8);
    }
    crc ^ 0xFFFF_FFFF
}
