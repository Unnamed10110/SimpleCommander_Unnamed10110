//! Non-elevated fallback index: a background walk of a directory tree using
//! the fast Win32 enumerator. Bounded so it cannot eat unbounded memory.

use parking_lot::RwLock;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

pub const MAX_ENTRIES: usize = 2_000_000;

struct Inner {
    /// (lower-cased name, directory id, is_dir)
    names: Vec<(Box<str>, u32, bool)>,
    /// Original-case names, parallel to `names`.
    display: Vec<Box<str>>,
    dirs: Vec<PathBuf>,
    complete: bool,
}

pub struct FallbackIndex {
    root: PathBuf,
    inner: RwLock<Inner>,
    stop: AtomicBool,
}

impl FallbackIndex {
    /// Start building in the background; returns immediately.
    pub fn start(root: PathBuf, on_done: impl Fn() + Send + 'static) -> Arc<Self> {
        let index = Arc::new(Self {
            root: root.clone(),
            inner: RwLock::new(Inner {
                names: Vec::new(),
                display: Vec::new(),
                dirs: Vec::new(),
                complete: false,
            }),
            stop: AtomicBool::new(false),
        });
        let this = index.clone();
        std::thread::Builder::new()
            .name("sc-fallback-index".into())
            .spawn(move || {
                let mut batch_names: Vec<(Box<str>, u32, bool)> = Vec::new();
                let mut batch_display: Vec<Box<str>> = Vec::new();
                let mut dir_ids: std::collections::HashMap<PathBuf, u32> =
                    std::collections::HashMap::new();
                let mut full = false;
                let _ = sc_shell::enumerate::enumerate_tree(&this.root, &mut |rel, entries| {
                    if this.stop.load(Ordering::Relaxed) {
                        return false;
                    }
                    let dir_id = match dir_ids.get(rel) {
                        Some(&id) => id,
                        None => {
                            let mut inner = this.inner.write();
                            let id = inner.dirs.len() as u32;
                            inner.dirs.push(this.root.join(rel));
                            dir_ids.insert(rel.to_path_buf(), id);
                            id
                        }
                    };
                    for e in entries {
                        batch_names.push((
                            e.name.to_lowercase().into_boxed_str(),
                            dir_id,
                            e.is_dir(),
                        ));
                        batch_display.push(e.name.into_boxed_str());
                    }
                    if batch_names.len() >= 8192 {
                        let mut inner = this.inner.write();
                        if inner.names.len() + batch_names.len() > MAX_ENTRIES {
                            full = true;
                        }
                        inner.names.append(&mut batch_names);
                        inner.display.append(&mut batch_display);
                    }
                    !full
                });
                let mut inner = this.inner.write();
                inner.names.append(&mut batch_names);
                inner.display.append(&mut batch_display);
                inner.complete = true;
                drop(inner);
                on_done();
            })
            .ok();
        index
    }

    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn len(&self) -> usize {
        self.inner.read().names.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn is_complete(&self) -> bool {
        self.inner.read().complete
    }

    pub fn search(&self, pred: &dyn Fn(&str) -> bool, name_suffix: Option<&str>, max: usize, out: &mut Vec<(PathBuf, bool)>) {
        let inner = self.inner.read();
        for (i, (lower, dir_id, is_dir)) in inner.names.iter().enumerate() {
            if out.len() >= max {
                return;
            }
            if let Some(suf) = name_suffix {
                if !lower.ends_with(suf) {
                    continue;
                }
            }
            let dir = &inner.dirs[*dir_id as usize];
            let full = dir.join(&*inner.display[i]);
            if pred(&full.to_string_lossy()) {
                out.push((full, *is_dir));
            }
        }
    }
}
