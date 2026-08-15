//! Archives-as-folders: transparently browse zip files. A "virtual" path is a
//! regular PathBuf that passes through a .zip file, e.g.
//! `C:\data\backup.zip\photos\2024`. The listing layer detects this and reads
//! the zip central directory instead of the filesystem.

use sc_core::entry::{ATTR_DIRECTORY, ATTR_READONLY};
use sc_core::FsEntry;
use std::collections::HashSet;
use std::io::Read;
use std::path::{Path, PathBuf};

/// If `path` points inside a zip file, split it into (zip_file, inner_path).
pub fn split_zip_path(path: &Path) -> Option<(PathBuf, String)> {
    let mut current = PathBuf::new();
    let mut components = path.components();
    while let Some(c) = components.next() {
        current.push(c);
        let is_zip = current
            .extension()
            .map(|e| e.eq_ignore_ascii_case("zip"))
            .unwrap_or(false);
        if is_zip && current.is_file() {
            let rest: PathBuf = components.collect();
            let inner = rest.to_string_lossy().replace('\\', "/");
            return Some((current, inner));
        }
    }
    None
}

/// Returns Some(listing) if `path` is a zip file or a folder inside one.
pub fn zip_listing(path: &Path) -> Option<Result<Vec<FsEntry>, String>> {
    let (zip_path, inner) = if path
        .extension()
        .map(|e| e.eq_ignore_ascii_case("zip"))
        .unwrap_or(false)
        && path.is_file()
    {
        (path.to_path_buf(), String::new())
    } else {
        split_zip_path(path)?
    };
    Some(list_zip_dir(&zip_path, &inner))
}

fn dos_time_to_filetime(dt: Option<zip::DateTime>) -> u64 {
    // Approximate: convert to unix seconds then to FILETIME ticks.
    let unix = dt
        .and_then(|d| {
            use std::convert::TryInto;
            let _: zip::DateTime = d;
            // zip::DateTime has year/month/day/hour/minute/second accessors.
            let days_from_civil = |y: i64, m: i64, d: i64| -> i64 {
                let y = if m <= 2 { y - 1 } else { y };
                let era = if y >= 0 { y } else { y - 399 } / 400;
                let yoe = y - era * 400;
                let mp = (m + 9) % 12;
                let doy = (153 * mp + 2) / 5 + d - 1;
                let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
                era * 146097 + doe - 719468
            };
            let days = days_from_civil(d.year() as i64, d.month() as i64, d.day() as i64);
            let secs =
                days * 86400 + d.hour() as i64 * 3600 + d.minute() as i64 * 60 + d.second() as i64;
            let _ = TryInto::<u64>::try_into(0u32);
            Some(secs)
        })
        .unwrap_or(0);
    if unix <= 0 {
        0
    } else {
        (unix as u64 + 11_644_473_600) * 10_000_000
    }
}

/// List the "directory" `inner` (using `/` separators, empty = root) of a zip.
fn list_zip_dir(zip_path: &Path, inner: &str) -> Result<Vec<FsEntry>, String> {
    let file = std::fs::File::open(zip_path).map_err(|e| e.to_string())?;
    let mut archive = zip::ZipArchive::new(file).map_err(|e| e.to_string())?;
    let prefix = if inner.is_empty() {
        String::new()
    } else {
        format!("{}/", inner.trim_matches('/'))
    };
    let mut files: Vec<FsEntry> = Vec::new();
    let mut dirs: HashSet<String> = HashSet::new();
    for i in 0..archive.len() {
        let entry = match archive.by_index_raw(i) {
            Ok(e) => e,
            Err(_) => continue,
        };
        let name = entry.name().trim_start_matches('/');
        let Some(rest) = name.strip_prefix(&prefix) else {
            continue;
        };
        if rest.is_empty() {
            continue;
        }
        match rest.find('/') {
            Some(slash) => {
                // Entry in a subdirectory: surface the first path segment.
                dirs.insert(rest[..slash].to_string());
            }
            None => {
                if entry.is_dir() {
                    dirs.insert(rest.trim_end_matches('/').to_string());
                } else {
                    files.push(FsEntry {
                        name: rest.to_string(),
                        size: entry.size(),
                        modified: dos_time_to_filetime(entry.last_modified()),
                        created: 0,
                        attributes: ATTR_READONLY,
                    });
                }
            }
        }
    }
    let mut out: Vec<FsEntry> = dirs
        .into_iter()
        .map(|d| FsEntry {
            name: d,
            size: 0,
            modified: 0,
            created: 0,
            attributes: ATTR_DIRECTORY | ATTR_READONLY,
        })
        .collect();
    out.append(&mut files);
    Ok(out)
}

/// Extract one file from a zip to a destination path.
pub fn extract_file(zip_path: &Path, inner: &str, dest: &Path) -> Result<(), String> {
    let file = std::fs::File::open(zip_path).map_err(|e| e.to_string())?;
    let mut archive = zip::ZipArchive::new(file).map_err(|e| e.to_string())?;
    let mut entry = archive
        .by_name(inner)
        .map_err(|e| format!("{inner}: {e}"))?;
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let mut out = std::fs::File::create(dest).map_err(|e| e.to_string())?;
    std::io::copy(&mut entry, &mut out).map_err(|e| e.to_string())?;
    Ok(())
}

/// Extract a file or directory subtree from a zip into `dest_dir`.
pub fn extract_selection(
    zip_path: &Path,
    inner: &str,
    is_dir: bool,
    dest_dir: &Path,
) -> Result<(), String> {
    let file = std::fs::File::open(zip_path).map_err(|e| e.to_string())?;
    let mut archive = zip::ZipArchive::new(file).map_err(|e| e.to_string())?;
    let base_name = inner.rsplit('/').next().unwrap_or(inner);
    if !is_dir {
        let mut entry = archive.by_name(inner).map_err(|e| e.to_string())?;
        let dest = dest_dir.join(base_name);
        let mut out = std::fs::File::create(&dest).map_err(|e| e.to_string())?;
        std::io::copy(&mut entry, &mut out).map_err(|e| e.to_string())?;
        return Ok(());
    }
    let prefix = format!("{}/", inner.trim_matches('/'));
    for i in 0..archive.len() {
        let mut entry = match archive.by_index(i) {
            Ok(e) => e,
            Err(_) => continue,
        };
        let name = entry.name().trim_start_matches('/').to_string();
        let Some(rest) = name.strip_prefix(&prefix) else {
            continue;
        };
        if rest.is_empty() || entry.is_dir() {
            continue;
        }
        let dest = dest_dir.join(base_name).join(rest.replace('/', "\\"));
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let mut out = std::fs::File::create(&dest).map_err(|e| e.to_string())?;
        std::io::copy(&mut entry, &mut out).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Extract a zip inner file into the temp dir and return the temp path
/// (used to open zip contents with associated programs).
pub fn extract_to_temp(zip_path: &Path, inner: &str) -> Result<PathBuf, String> {
    let name = inner.rsplit('/').next().unwrap_or("file");
    let dir = std::env::temp_dir().join("SimpleCommander").join("zip");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let dest = dir.join(name);
    extract_file(zip_path, inner, &dest)?;
    Ok(dest)
}

#[allow(dead_code)]
fn _use_read(_r: &mut dyn Read) {}
