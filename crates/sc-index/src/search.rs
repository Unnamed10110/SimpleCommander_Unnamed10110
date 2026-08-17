//! Search facade: owns per-volume MFT indexes (elevated) or a fallback
//! tree index, and provides name search plus scoped content search.

use crate::fallback::FallbackIndex;
use crate::mft::MftIndex;
use parking_lot::RwLock;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IndexStatus {
    Building,
    Ready { entries: usize, mft: bool },
    Unavailable,
}

pub struct IndexService {
    mft: RwLock<Vec<Arc<MftIndex>>>,
    fallback: RwLock<Option<Arc<FallbackIndex>>>,
    status: RwLock<IndexStatus>,
}

impl IndexService {
    /// Spawn index construction for all fixed NTFS volumes. Never blocks.
    /// Pass `enabled = false` to skip building (status stays Unavailable).
    pub fn start(enabled: bool, notify: impl Fn() + Send + Sync + Clone + 'static) -> Arc<Self> {
        let service = Arc::new(Self {
            mft: RwLock::new(Vec::new()),
            fallback: RwLock::new(None),
            status: RwLock::new(if enabled {
                IndexStatus::Building
            } else {
                IndexStatus::Unavailable
            }),
        });
        std::thread::Builder::new()
            .name("sc-everything-probe".into())
            .spawn(|| {
                sc_shell::everything::warmup();
            })
            .ok();
        if !enabled {
            return service;
        }
        let this = service.clone();
        std::thread::Builder::new()
            .name("sc-index-build".into())
            .spawn(move || {
                let volumes = sc_shell::volumes::list_volumes();
                let mut any_mft = false;
                for v in volumes
                    .iter()
                    .filter(|v| v.drive_type == sc_shell::volumes::DriveType::Fixed)
                {
                    let letter = v
                        .root
                        .to_string_lossy()
                        .chars()
                        .next()
                        .unwrap_or('C')
                        .to_ascii_uppercase();
                    match MftIndex::build(letter) {
                        Ok(index) => {
                            this.mft.write().push(index);
                            any_mft = true;
                        }
                        Err(_) => { /* likely not elevated or not NTFS */ }
                    }
                }
                if any_mft {
                    let entries = this.mft.read().iter().map(|i| i.len()).sum();
                    *this.status.write() = IndexStatus::Ready { entries, mft: true };
                    notify();
                } else {
                    // Fallback: index the user profile tree without elevation.
                    let home = sc_core::state::dirs_home();
                    let this2 = this.clone();
                    let notify2 = notify.clone();
                    let fb = FallbackIndex::start(home, move || {
                        let entries =
                            this2.fallback.read().as_ref().map(|f| f.len()).unwrap_or(0);
                        *this2.status.write() = IndexStatus::Ready { entries, mft: false };
                        notify2();
                    });
                    *this.fallback.write() = Some(fb);
                }
            })
            .ok();
        service
    }

    pub fn status(&self) -> IndexStatus {
        self.status.read().clone()
    }

    /// Instant name/path search. Uses Everything when it is installed/running.
    /// When `dirs_only` is set, only folders are returned (and count toward `max`).
    /// `near` ranks hits closest to that directory first (current folder).
    /// `quick` does a single Everything query (for the Ctrl+P palette).
    pub fn search_names(
        &self,
        query: &str,
        max: usize,
        scope: Option<&Path>,
        dirs_only: bool,
        near: Option<&Path>,
        quick: bool,
        should_stop: &dyn Fn() -> bool,
    ) -> Vec<(PathBuf, bool)> {
        let query = query.trim();
        if query.is_empty() {
            return Vec::new();
        }
        let everything_query = if dirs_only && !query_has_folder_filter(query) {
            format!("folder: {query}")
        } else {
            query.to_string()
        };
        let max = max.max(1);
        let origin = near.or(scope);
        let fetch = if quick {
            max
        } else {
            max.saturating_mul(3).clamp(max, 2000)
        };

        if let Some(hits) = everything_hits(&everything_query, max, fetch, scope, origin, quick) {
            return hits;
        }
        if should_stop() {
            return Vec::new();
        }
        if let Some(dir) = scope {
            let mut hits = search_tree(dir, query, fetch, dirs_only, should_stop);
            if let Some(origin) = origin {
                rank_by_proximity(&mut hits, origin);
            }
            hits.truncate(max);
            return hits;
        }
        let q = sc_core::query::Query::parse(query);
        let suffix = q.required_name_suffix();
        let pred = |path: &str| q.matches(path);
        let mut hits = self.search_pred(&pred, suffix.as_deref(), fetch, dirs_only);
        if let Some(origin) = origin {
            rank_by_proximity(&mut hits, origin);
        }
        hits.truncate(max);
        hits
    }

    fn search_pred(
        &self,
        pred: &dyn Fn(&str) -> bool,
        name_suffix: Option<&str>,
        max: usize,
        dirs_only: bool,
    ) -> Vec<(PathBuf, bool)> {
        let mut out = Vec::new();
        for index in self.mft.read().iter() {
            index.search(pred, name_suffix, max, dirs_only, &mut out);
            if out.len() >= max {
                return out;
            }
        }
        if let Some(fb) = self.fallback.read().as_ref() {
            fb.search(pred, name_suffix, max, dirs_only, &mut out);
        }
        out
    }

    pub fn shutdown(&self) {
        for i in self.mft.read().iter() {
            i.stop();
        }
        if let Some(f) = self.fallback.read().as_ref() {
            f.stop();
        }
    }
}

fn search_tree(
    scope: &Path,
    query: &str,
    max: usize,
    dirs_only: bool,
    should_stop: &dyn Fn() -> bool,
) -> Vec<(PathBuf, bool)> {
    let q = sc_core::query::Query::parse(query);
    let mut results = Vec::new();
    let _ = sc_shell::enumerate::enumerate_tree(scope, &mut |rel, batch| {
        if should_stop() {
            return false;
        }
        for e in batch {
            if dirs_only && !e.is_dir() {
                continue;
            }
            let full = scope.join(rel).join(&e.name);
            if q.matches(&full.to_string_lossy()) {
                results.push((full, e.is_dir()));
                if results.len() >= max {
                    return false;
                }
            }
        }
        true
    });
    results
}

fn query_has_folder_filter(query: &str) -> bool {
    query
        .split_whitespace()
        .any(|t| t.eq_ignore_ascii_case("folder:") || t.to_ascii_lowercase().starts_with("folder:"))
}

fn everything_hits(
    query: &str,
    max: usize,
    fetch: usize,
    scope: Option<&Path>,
    origin: Option<&Path>,
    quick: bool,
) -> Option<Vec<(PathBuf, bool)>> {
    if quick {
        let mut hits = sc_shell::everything::search(query, fetch)?;
        if let Some(origin) = origin {
            rank_by_proximity(&mut hits, origin);
        }
        hits.truncate(max);
        return Some(hits);
    }
    if let Some(dir) = scope {
        let mut hits = sc_shell::everything::search_in(dir, query, fetch)?;
        if let Some(origin) = origin {
            rank_by_proximity(&mut hits, origin);
        }
        hits.truncate(max);
        return Some(hits);
    }

    let mut hits = Vec::new();
    if let Some(dir) = origin {
        if let Some(local) = sc_shell::everything::search_in(dir, query, fetch) {
            hits = local;
        }
    }
    match sc_shell::everything::search(query, fetch) {
        Some(global) => merge_hits(&mut hits, global),
        None if hits.is_empty() => return None,
        None => {}
    }
    if let Some(origin) = origin {
        rank_by_proximity(&mut hits, origin);
    }
    hits.truncate(max);
    Some(hits)
}

fn merge_hits(into: &mut Vec<(PathBuf, bool)>, extra: Vec<(PathBuf, bool)>) {
    let mut seen: HashSet<String> = into
        .iter()
        .map(|(p, _)| p.to_string_lossy().to_ascii_lowercase())
        .collect();
    for hit in extra {
        let key = hit.0.to_string_lossy().to_ascii_lowercase();
        if seen.insert(key) {
            into.push(hit);
        }
    }
}

/// Lower score = closer to `origin`. Files in the current folder rank first,
/// then nested children, then siblings, then farther branches.
fn rank_by_proximity(hits: &mut Vec<(PathBuf, bool)>, origin: &Path) {
    hits.sort_by(|(a, _), (b, _)| {
        path_distance(origin, a)
            .cmp(&path_distance(origin, b))
            .then_with(|| a.as_os_str().len().cmp(&b.as_os_str().len()))
            .then_with(|| a.cmp(b))
    });
}

fn path_distance(origin: &Path, hit: &Path) -> (u8, u32) {
    let origin = path_comps(origin);
    let hit_dir = path_comps(hit.parent().unwrap_or(hit));
    if hit_dir.len() >= origin.len() && hit_dir[..origin.len()] == origin[..] {
        return (0, (hit_dir.len() - origin.len()) as u32);
    }
    let common = origin
        .iter()
        .zip(hit_dir.iter())
        .take_while(|(a, b)| a == b)
        .count();
    (
        1,
        (origin.len().saturating_sub(common) + hit_dir.len().saturating_sub(common)) as u32,
    )
}

fn path_comps(p: &Path) -> Vec<String> {
    p.components()
        .filter_map(|c| match c {
            std::path::Component::Prefix(pre) => {
                Some(pre.as_os_str().to_string_lossy().to_ascii_lowercase())
            }
            std::path::Component::RootDir => Some("\\".into()),
            std::path::Component::Normal(s) => Some(s.to_string_lossy().to_ascii_lowercase()),
            _ => None,
        })
        .collect()
}

/// Scoped content search: walk `scope`, scan files for `needle`
/// (case-insensitive), capped by file size and match count.
pub fn content_search(
    scope: &Path,
    needle: &str,
    max_results: usize,
    max_file_size: u64,
    should_stop: &dyn Fn() -> bool,
) -> Vec<PathBuf> {
    let needle_lower = needle.to_lowercase();
    let needle_bytes = needle_lower.as_bytes();
    if needle_bytes.is_empty() {
        return Vec::new();
    }
    let mut results = Vec::new();
    let _ = sc_shell::enumerate::enumerate_tree(scope, &mut |rel, entries| {
        for e in entries {
            if results.len() >= max_results || should_stop() {
                return false;
            }
            if e.is_dir() || e.size > max_file_size {
                continue;
            }
            let path = scope.join(rel).join(&e.name);
            if let Ok(data) = std::fs::read(&path) {
                if contains_ci(&data, needle_bytes) {
                    results.push(path);
                }
            }
        }
        !(results.len() >= max_results || should_stop())
    });
    results
}

/// ASCII case-insensitive byte search (memchr-accelerated by the stdlib).
fn contains_ci(haystack: &[u8], needle_lower: &[u8]) -> bool {
    if needle_lower.is_empty() || haystack.len() < needle_lower.len() {
        return false;
    }
    let first = needle_lower[0];
    let first_upper = first.to_ascii_uppercase();
    let end = haystack.len() - needle_lower.len();
    let mut i = 0;
    while i <= end {
        let b = haystack[i];
        if b == first || b == first_upper {
            if haystack[i..i + needle_lower.len()]
                .iter()
                .zip(needle_lower)
                .all(|(&h, &n)| h.to_ascii_lowercase() == n)
            {
                return true;
            }
        }
        i += 1;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn closer_paths_rank_first() {
        let origin = PathBuf::from(r"C:\work\proj");
        let mut hits = vec![
            (PathBuf::from(r"C:\other\readme.txt"), false),
            (PathBuf::from(r"C:\work\proj\src\lib.rs"), false),
            (PathBuf::from(r"C:\work\proj\readme.txt"), false),
            (PathBuf::from(r"C:\work\notes.txt"), false),
        ];
        rank_by_proximity(&mut hits, &origin);
        assert_eq!(hits[0].0, PathBuf::from(r"C:\work\proj\readme.txt"));
        assert_eq!(hits[1].0, PathBuf::from(r"C:\work\proj\src\lib.rs"));
        assert_eq!(hits[2].0, PathBuf::from(r"C:\work\notes.txt"));
        assert_eq!(hits[3].0, PathBuf::from(r"C:\other\readme.txt"));
    }
}
