//! One-level (optional recursive) folder compare between the two panes.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompareKind {
    Same,
    LeftOnly,
    RightOnly,
    NewerLeft,
    NewerRight,
}

impl CompareKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Same => "Same",
            Self::LeftOnly => "Left only",
            Self::RightOnly => "Right only",
            Self::NewerLeft => "Newer left",
            Self::NewerRight => "Newer right",
        }
    }
}

#[derive(Clone, Debug)]
pub struct CompareRow {
    pub rel: String,
    pub kind: CompareKind,
    pub left: Option<PathBuf>,
    pub right: Option<PathBuf>,
    pub is_dir: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompareFilter {
    All,
    LeftOnly,
    RightOnly,
    Different,
    Same,
}

impl CompareFilter {
    pub fn label(self) -> &'static str {
        match self {
            Self::All => "All",
            Self::LeftOnly => "Left only",
            Self::RightOnly => "Right only",
            Self::Different => "Different",
            Self::Same => "Same",
        }
    }

    pub fn matches(self, kind: CompareKind) -> bool {
        match self {
            Self::All => true,
            Self::LeftOnly => kind == CompareKind::LeftOnly,
            Self::RightOnly => kind == CompareKind::RightOnly,
            Self::Same => kind == CompareKind::Same,
            Self::Different => matches!(
                kind,
                CompareKind::NewerLeft | CompareKind::NewerRight | CompareKind::LeftOnly | CompareKind::RightOnly
            ),
        }
    }
}

pub struct FolderCompareState {
    pub open: bool,
    pub left: PathBuf,
    pub right: PathBuf,
    pub include_subfolders: bool,
    pub filter: CompareFilter,
    pub rows: Vec<CompareRow>,
    pub running: bool,
    pub query_id: u64,
    pub selected: HashSet<usize>,
}

impl Default for FolderCompareState {
    fn default() -> Self {
        Self {
            open: false,
            left: PathBuf::new(),
            right: PathBuf::new(),
            include_subfolders: false,
            filter: CompareFilter::All,
            rows: Vec::new(),
            running: false,
            query_id: 0,
            selected: HashSet::new(),
        }
    }
}

#[derive(Clone, Copy)]
struct Meta {
    size: u64,
    mtime: u64,
    is_dir: bool,
}

fn collect(dir: &Path, recursive: bool) -> BTreeMap<String, (PathBuf, Meta)> {
    let mut out = BTreeMap::new();
    walk(dir, "", recursive, &mut out);
    out
}

fn walk(dir: &Path, rel: &str, recursive: bool, out: &mut BTreeMap<String, (PathBuf, Meta)>) {
    let rd = match std::fs::read_dir(dir) {
        Ok(rd) => rd,
        Err(_) => return,
    };
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let path = e.path();
        let ok_rel = if rel.is_empty() {
            name.clone()
        } else {
            format!("{rel}\\{name}")
        };
        let meta = match e.metadata() {
            Ok(m) => m,
            Err(_) => continue,
        };
        let is_dir = meta.is_dir();
        let mtime = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs())
            .unwrap_or(0);
        out.insert(
            ok_rel.clone(),
            (
                path.clone(),
                Meta {
                    size: meta.len(),
                    mtime,
                    is_dir,
                },
            ),
        );
        if recursive && is_dir {
            walk(&path, &ok_rel, true, out);
        }
    }
}

pub fn compare_folders(left: &Path, right: &Path, recursive: bool) -> Vec<CompareRow> {
    let lmap = collect(left, recursive);
    let rmap = collect(right, recursive);
    let mut names: Vec<String> = lmap.keys().chain(rmap.keys()).cloned().collect();
    names.sort();
    names.dedup();
    let mut rows = Vec::with_capacity(names.len());
    for rel in names {
        let l = lmap.get(&rel);
        let r = rmap.get(&rel);
        match (l, r) {
            (Some((lp, lm)), None) => rows.push(CompareRow {
                rel,
                kind: CompareKind::LeftOnly,
                left: Some(lp.clone()),
                right: None,
                is_dir: lm.is_dir,
            }),
            (None, Some((rp, rm))) => rows.push(CompareRow {
                rel,
                kind: CompareKind::RightOnly,
                left: None,
                right: Some(rp.clone()),
                is_dir: rm.is_dir,
            }),
            (Some((lp, lm)), Some((rp, rm))) => {
                let kind = if lm.is_dir && rm.is_dir {
                    CompareKind::Same
                } else if lm.size == rm.size && lm.mtime == rm.mtime {
                    CompareKind::Same
                } else if lm.mtime > rm.mtime {
                    CompareKind::NewerLeft
                } else if rm.mtime > lm.mtime {
                    CompareKind::NewerRight
                } else {
                    // same mtime, different size
                    CompareKind::NewerLeft
                };
                rows.push(CompareRow {
                    rel,
                    kind,
                    left: Some(lp.clone()),
                    right: Some(rp.clone()),
                    is_dir: lm.is_dir || rm.is_dir,
                });
            }
            (None, None) => {}
        }
    }
    rows
}
