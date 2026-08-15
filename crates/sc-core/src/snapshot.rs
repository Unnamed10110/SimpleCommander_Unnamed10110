use crate::entry::FsEntry;
use std::path::PathBuf;
use std::sync::Arc;

/// An immutable snapshot of a directory listing. Produced on worker threads,
/// swapped into pane state atomically. The UI never mutates it.
#[derive(Clone, Debug)]
pub struct DirSnapshot {
    pub path: PathBuf,
    pub entries: Arc<Vec<FsEntry>>,
    /// Monotonic per-tab generation; stale results are dropped by the UI.
    pub generation: u64,
    /// False while enumeration batches are still streaming in.
    pub complete: bool,
    pub error: Option<String>,
    pub dir_count: usize,
    pub file_count: usize,
    pub file_bytes: u64,
}

impl DirSnapshot {
    pub fn empty(path: PathBuf, generation: u64) -> Self {
        Self {
            path,
            entries: Arc::new(Vec::new()),
            generation,
            complete: false,
            error: None,
            dir_count: 0,
            file_count: 0,
            file_bytes: 0,
        }
    }

    pub fn recompute_counts(&mut self) {
        let mut dirs = 0usize;
        let mut file_bytes = 0u64;
        for e in self.entries.iter() {
            if e.is_dir() {
                dirs += 1;
            } else {
                file_bytes += e.size;
            }
        }
        self.dir_count = dirs;
        self.file_count = self.entries.len() - dirs;
        self.file_bytes = file_bytes;
    }

    pub fn total_size(&self) -> u64 {
        self.file_bytes
    }

    pub fn counts(&self) -> (usize, usize) {
        (self.dir_count, self.file_count)
    }
}
