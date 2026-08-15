//! Search facade: owns per-volume MFT indexes (elevated) or a fallback
//! tree index, and provides name search plus scoped content search.

use crate::fallback::FallbackIndex;
use crate::mft::MftIndex;
use parking_lot::RwLock;
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
    pub fn search_names(
        &self,
        query: &str,
        max: usize,
        scope: Option<&Path>,
        should_stop: &dyn Fn() -> bool,
    ) -> Vec<(PathBuf, bool)> {
        let query = query.trim();
        if query.is_empty() {
            return Vec::new();
        }
        if let Some(dir) = scope {
            if let Some(hits) = sc_shell::everything::search_in(dir, query, max) {
                return hits;
            }
            return search_tree(dir, query, max, should_stop);
        }
        if let Some(hits) = sc_shell::everything::search(query, max) {
            return hits;
        }
        let q = sc_core::query::Query::parse(query);
        let suffix = q.required_name_suffix();
        let pred = |path: &str| q.matches(path);
        self.search_pred(&pred, suffix.as_deref(), max)
    }

    fn search_pred(
        &self,
        pred: &dyn Fn(&str) -> bool,
        name_suffix: Option<&str>,
        max: usize,
    ) -> Vec<(PathBuf, bool)> {
        let mut out = Vec::new();
        for index in self.mft.read().iter() {
            index.search(pred, name_suffix, max, &mut out);
            if out.len() >= max {
                return out;
            }
        }
        if let Some(fb) = self.fallback.read().as_ref() {
            fb.search(pred, name_suffix, max, &mut out);
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
    should_stop: &dyn Fn() -> bool,
) -> Vec<(PathBuf, bool)> {
    let q = sc_core::query::Query::parse(query);
    let mut results = Vec::new();
    let _ = sc_shell::enumerate::enumerate_tree(scope, &mut |rel, batch| {
        if should_stop() {
            return false;
        }
        for e in batch {
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
