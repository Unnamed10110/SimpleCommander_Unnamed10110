//! Background file-operation queue. One dedicated worker thread executes
//! operations sequentially (XYplorer-style background transfer queue) and
//! streams progress events to the UI. Copies use `CopyFileExW` for maximum
//! throughput with progress; recycle-bin deletes go through `IFileOperation`.

use crossbeam_channel::{unbounded, Receiver, Sender};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;
use windows::core::PCWSTR;
use windows::Win32::Foundation::HWND;
use windows::Win32::Storage::FileSystem::{
    CopyFileExW, MoveFileWithProgressW, COPYFILE_FLAGS, LPPROGRESS_ROUTINE_CALLBACK_REASON,
    MOVEFILE_COPY_ALLOWED, MOVEFILE_REPLACE_EXISTING,
};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_ALL, COINIT_APARTMENTTHREADED,
};
use windows::Win32::UI::Shell::{
    FileOperation, IFileOperation, IShellItem, SHCreateItemFromParsingName, FOF_ALLOWUNDO,
    FOF_NOCONFIRMATION, FOF_NO_UI,
};

use std::os::windows::ffi::OsStrExt;

#[derive(Clone, Debug)]
pub enum Operation {
    Copy { sources: Vec<PathBuf>, dest_dir: PathBuf },
    Move { sources: Vec<PathBuf>, dest_dir: PathBuf },
    Delete { paths: Vec<PathBuf>, recycle: bool },
    Rename { from: PathBuf, to: PathBuf },
    NewFolder { path: PathBuf },
    NewFile { path: PathBuf },
}

impl Operation {
    pub fn label(&self) -> String {
        match self {
            Operation::Copy { sources, dest_dir } => {
                format!("Copy {} item(s) to {}", sources.len(), dest_dir.display())
            }
            Operation::Move { sources, dest_dir } => {
                format!("Move {} item(s) to {}", sources.len(), dest_dir.display())
            }
            Operation::Delete { paths, recycle } => {
                if *recycle {
                    format!("Recycle {} item(s)", paths.len())
                } else {
                    format!("Delete {} item(s)", paths.len())
                }
            }
            Operation::Rename { from, to } => format!(
                "Rename {} to {}",
                from.file_name().unwrap_or_default().to_string_lossy(),
                to.file_name().unwrap_or_default().to_string_lossy()
            ),
            Operation::NewFolder { path } => format!("New folder {}", path.display()),
            Operation::NewFile { path } => format!("New file {}", path.display()),
        }
    }
}

#[derive(Clone, Debug)]
pub enum OpEvent {
    Started { op_id: u64, label: String, total_bytes: u64, total_files: u64 },
    Progress {
        op_id: u64,
        done_bytes: u64,
        total_bytes: u64,
        done_files: u64,
        total_files: u64,
        current: String,
    },
    Conflict { op_id: u64, source: PathBuf, dest: PathBuf },
    Done { op_id: u64, undo: Option<UndoAction>, refresh: Vec<PathBuf> },
    Failed { op_id: u64, error: String },
    Cancelled { op_id: u64 },
}

/// Inverse action recorded for undo.
#[derive(Clone, Debug)]
pub enum UndoAction {
    DeletePaths(Vec<PathBuf>),
    MoveBack { pairs: Vec<(PathBuf, PathBuf)> },
    RenameBack { from: PathBuf, to: PathBuf },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConflictResolution {
    Overwrite,
    Skip,
    AutoRename,
    Cancel,
}

const PAUSE_NONE: u8 = 0;
const PAUSE_PAUSED: u8 = 1;
const PAUSE_CANCELLED: u8 = 2;

pub struct OpEngine {
    submit: Sender<(u64, Operation)>,
    pub events: Receiver<OpEvent>,
    next_id: std::sync::atomic::AtomicU64,
    state: Arc<AtomicU8>,
    conflict_tx: Sender<(ConflictResolution, bool)>,
}

impl OpEngine {
    pub fn new(notify: impl Fn() + Send + Sync + 'static) -> Self {
        let (submit_tx, submit_rx) = unbounded::<(u64, Operation)>();
        let (event_tx, event_rx) = unbounded::<OpEvent>();
        let (conflict_tx, conflict_rx) = unbounded::<(ConflictResolution, bool)>();
        let state = Arc::new(AtomicU8::new(PAUSE_NONE));
        let worker_state = state.clone();
        std::thread::Builder::new()
            .name("sc-ops".into())
            .spawn(move || {
                unsafe {
                    let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
                }
                let mut worker = Worker {
                    events: event_tx,
                    conflicts: conflict_rx,
                    state: worker_state,
                    notify: Box::new(notify),
                };
                while let Ok((op_id, op)) = submit_rx.recv() {
                    worker.state.store(PAUSE_NONE, Ordering::SeqCst);
                    worker.run(op_id, op);
                }
                unsafe { CoUninitialize() };
            })
            .expect("spawn ops worker");
        Self {
            submit: submit_tx,
            events: event_rx,
            next_id: std::sync::atomic::AtomicU64::new(1),
            state,
            conflict_tx,
        }
    }

    pub fn submit(&self, op: Operation) -> u64 {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let _ = self.submit.send((id, op));
        id
    }

    pub fn pause(&self) {
        let _ = self.state.compare_exchange(
            PAUSE_NONE,
            PAUSE_PAUSED,
            Ordering::SeqCst,
            Ordering::SeqCst,
        );
    }

    pub fn resume(&self) {
        let _ = self.state.compare_exchange(
            PAUSE_PAUSED,
            PAUSE_NONE,
            Ordering::SeqCst,
            Ordering::SeqCst,
        );
    }

    pub fn cancel(&self) {
        self.state.store(PAUSE_CANCELLED, Ordering::SeqCst);
    }

    pub fn is_paused(&self) -> bool {
        self.state.load(Ordering::SeqCst) == PAUSE_PAUSED
    }

    /// Answer a pending conflict prompt. `apply_to_all` suppresses further prompts.
    pub fn resolve_conflict(&self, res: ConflictResolution, apply_to_all: bool) {
        let _ = self.conflict_tx.send((res, apply_to_all));
    }
}

struct Worker {
    events: Sender<OpEvent>,
    conflicts: Receiver<(ConflictResolution, bool)>,
    state: Arc<AtomicU8>,
    notify: Box<dyn Fn() + Send + Sync>,
}

struct Totals {
    bytes: u64,
    files: u64,
}

impl Worker {
    fn send(&self, ev: OpEvent) {
        let _ = self.events.send(ev);
        (self.notify)();
    }

    fn cancelled(&self) -> bool {
        self.state.load(Ordering::SeqCst) == PAUSE_CANCELLED
    }

    /// Block while paused; returns false if cancelled.
    fn pause_point(&self) -> bool {
        loop {
            match self.state.load(Ordering::SeqCst) {
                PAUSE_PAUSED => std::thread::sleep(std::time::Duration::from_millis(50)),
                PAUSE_CANCELLED => return false,
                _ => return true,
            }
        }
    }

    fn run(&mut self, op_id: u64, op: Operation) {
        let refresh = refresh_targets(&op);
        let result = match op {
            Operation::Copy { sources, dest_dir } => self.copy_or_move(op_id, sources, dest_dir, false),
            Operation::Move { sources, dest_dir } => self.copy_or_move(op_id, sources, dest_dir, true),
            Operation::Delete { paths, recycle } => self.delete(op_id, paths, recycle),
            Operation::Rename { from, to } => self.rename(op_id, from, to),
            Operation::NewFolder { path } => {
                self.send(OpEvent::Started {
                    op_id,
                    label: format!("New folder {}", path.display()),
                    total_bytes: 0,
                    total_files: 1,
                });
                std::fs::create_dir(&path)
                    .map(|_| Some(UndoAction::DeletePaths(vec![path])))
                    .map_err(|e| e.to_string())
            }
            Operation::NewFile { path } => {
                self.send(OpEvent::Started {
                    op_id,
                    label: format!("New file {}", path.display()),
                    total_bytes: 0,
                    total_files: 1,
                });
                std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&path)
                    .map(|_| Some(UndoAction::DeletePaths(vec![path])))
                    .map_err(|e| e.to_string())
            }
        };
        match result {
            Ok(undo) => self.send(OpEvent::Done { op_id, undo, refresh }),
            Err(e) if e == "__cancelled__" => self.send(OpEvent::Cancelled { op_id }),
            Err(e) => self.send(OpEvent::Failed { op_id, error: e }),
        }
    }

    fn rename(&self, op_id: u64, from: PathBuf, to: PathBuf) -> Result<Option<UndoAction>, String> {
        self.send(OpEvent::Started {
            op_id,
            label: format!("Rename to {}", to.file_name().unwrap_or_default().to_string_lossy()),
            total_bytes: 0,
            total_files: 1,
        });
        std::fs::rename(&from, &to).map_err(|e| e.to_string())?;
        Ok(Some(UndoAction::RenameBack { from: to, to: from }))
    }

    fn copy_or_move(
        &mut self,
        op_id: u64,
        sources: Vec<PathBuf>,
        dest_dir: PathBuf,
        is_move: bool,
    ) -> Result<Option<UndoAction>, String> {
        let totals = scan_totals(&sources);
        self.send(OpEvent::Started {
            op_id,
            label: if is_move {
                format!("Moving {} item(s)", sources.len())
            } else {
                format!("Copying {} item(s)", sources.len())
            },
            total_bytes: totals.bytes,
            total_files: totals.files,
        });
        let mut ctx = TransferCtx {
            worker: self,
            op_id,
            done_bytes: 0,
            done_files: 0,
            totals,
            sticky_resolution: None,
            created: Vec::new(),
            moved_pairs: Vec::new(),
        };
        for src in &sources {
            let name = src.file_name().ok_or("invalid source path")?;
            let dest = dest_dir.join(name);
            if src == &dest {
                continue;
            }
            ctx.transfer(src, &dest, is_move)?;
        }
        Ok(if is_move {
            Some(UndoAction::MoveBack { pairs: ctx.moved_pairs })
        } else if ctx.created.is_empty() {
            None
        } else {
            Some(UndoAction::DeletePaths(ctx.created))
        })
    }

    fn delete(&self, op_id: u64, paths: Vec<PathBuf>, recycle: bool) -> Result<Option<UndoAction>, String> {
        self.send(OpEvent::Started {
            op_id,
            label: format!("Deleting {} item(s)", paths.len()),
            total_bytes: 0,
            total_files: paths.len() as u64,
        });
        if recycle {
            recycle_via_shell(&paths)?;
        } else {
            for (i, p) in paths.iter().enumerate() {
                if !self.pause_point() {
                    return Err("__cancelled__".into());
                }
                let meta = std::fs::symlink_metadata(p).map_err(|e| e.to_string())?;
                if meta.is_dir() {
                    std::fs::remove_dir_all(p).map_err(|e| e.to_string())?;
                } else {
                    std::fs::remove_file(p).map_err(|e| e.to_string())?;
                }
                self.send(OpEvent::Progress {
                    op_id,
                    done_bytes: 0,
                    total_bytes: 0,
                    done_files: i as u64 + 1,
                    total_files: paths.len() as u64,
                    current: p.display().to_string(),
                });
            }
        }
        Ok(None)
    }

    /// Ask the UI to resolve a conflict; blocks the worker until answered.
    fn ask_conflict(&self, op_id: u64, source: &Path, dest: &Path) -> (ConflictResolution, bool) {
        self.send(OpEvent::Conflict {
            op_id,
            source: source.to_path_buf(),
            dest: dest.to_path_buf(),
        });
        self.conflicts
            .recv()
            .unwrap_or((ConflictResolution::Cancel, false))
    }
}

struct TransferCtx<'a> {
    worker: &'a mut Worker,
    op_id: u64,
    done_bytes: u64,
    done_files: u64,
    totals: Totals,
    sticky_resolution: Option<ConflictResolution>,
    /// Top-level destinations created by a copy (for undo).
    created: Vec<PathBuf>,
    /// (original, new) pairs for move undo.
    moved_pairs: Vec<(PathBuf, PathBuf)>,
}

impl TransferCtx<'_> {
    fn transfer(&mut self, src: &Path, dest: &Path, is_move: bool) -> Result<(), String> {
        if !self.worker.pause_point() {
            return Err("__cancelled__".into());
        }
        let mut dest = dest.to_path_buf();
        if dest.exists() {
            let res = match self.sticky_resolution {
                Some(r) => r,
                None => {
                    let (r, all) = self.worker.ask_conflict(self.op_id, src, &dest);
                    if all {
                        self.sticky_resolution = Some(r);
                    }
                    r
                }
            };
            match res {
                ConflictResolution::Cancel => return Err("__cancelled__".into()),
                ConflictResolution::Skip => return Ok(()),
                ConflictResolution::AutoRename => dest = auto_rename(&dest),
                ConflictResolution::Overwrite => {}
            }
        }
        if is_move {
            self.move_one(src, &dest)?;
            self.moved_pairs.push((src.to_path_buf(), dest.clone()));
        } else {
            self.copy_recursive(src, &dest)?;
            self.created.push(dest.clone());
        }
        Ok(())
    }

    fn move_one(&mut self, src: &Path, dest: &Path) -> Result<(), String> {
        let src_w = wide(src);
        let dest_w = wide(dest);
        unsafe {
            MoveFileWithProgressW(
                PCWSTR::from_raw(src_w.as_ptr()),
                PCWSTR::from_raw(dest_w.as_ptr()),
                None,
                None,
                MOVEFILE_COPY_ALLOWED | MOVEFILE_REPLACE_EXISTING,
            )
        }
        .map_err(|e| format!("{}: {}", src.display(), e.message()))?;
        self.done_files += 1;
        self.progress(src);
        Ok(())
    }

    fn copy_recursive(&mut self, src: &Path, dest: &Path) -> Result<(), String> {
        if !self.worker.pause_point() {
            return Err("__cancelled__".into());
        }
        let meta = std::fs::symlink_metadata(src).map_err(|e| e.to_string())?;
        if meta.is_dir() {
            std::fs::create_dir_all(dest).map_err(|e| e.to_string())?;
            let rd = std::fs::read_dir(src).map_err(|e| e.to_string())?;
            for entry in rd.flatten() {
                let name = entry.file_name();
                self.copy_recursive(&src.join(&name), &dest.join(&name))?;
            }
            Ok(())
        } else {
            self.copy_file(src, dest, meta.len())
        }
    }

    fn copy_file(&mut self, src: &Path, dest: &Path, size: u64) -> Result<(), String> {
        let src_w = wide(src);
        let dest_w = wide(dest);
        let mut cancel = windows::core::BOOL(0);
        // Progress callback context: check pause/cancel between chunks.
        struct CbCtx {
            state: Arc<AtomicU8>,
        }
        unsafe extern "system" fn progress_cb(
            _total: i64,
            _transferred: i64,
            _stream_size: i64,
            _stream_transferred: i64,
            _stream: u32,
            _reason: LPPROGRESS_ROUTINE_CALLBACK_REASON,
            _hsrc: windows::Win32::Foundation::HANDLE,
            _hdst: windows::Win32::Foundation::HANDLE,
            data: *const core::ffi::c_void,
        ) -> windows::Win32::Storage::FileSystem::COPYPROGRESSROUTINE_PROGRESS {
            let ctx = unsafe { &*(data as *const CbCtx) };
            loop {
                match ctx.state.load(Ordering::SeqCst) {
                    PAUSE_PAUSED => std::thread::sleep(std::time::Duration::from_millis(50)),
                    // PROGRESS_CANCEL
                    PAUSE_CANCELLED => {
                        return windows::Win32::Storage::FileSystem::COPYPROGRESSROUTINE_PROGRESS(1)
                    }
                    // PROGRESS_CONTINUE
                    _ => return windows::Win32::Storage::FileSystem::COPYPROGRESSROUTINE_PROGRESS(0),
                }
            }
        }
        let ctx = CbCtx { state: self.worker.state.clone() };
        unsafe {
            CopyFileExW(
                PCWSTR::from_raw(src_w.as_ptr()),
                PCWSTR::from_raw(dest_w.as_ptr()),
                Some(progress_cb),
                Some(&ctx as *const CbCtx as *const _),
                Some(&mut cancel),
                COPYFILE_FLAGS(0),
            )
        }
        .map_err(|e| {
            if self.worker.cancelled() {
                "__cancelled__".to_string()
            } else {
                format!("{}: {}", src.display(), e.message())
            }
        })?;
        self.done_bytes += size;
        self.done_files += 1;
        self.progress(src);
        Ok(())
    }

    fn progress(&self, current: &Path) {
        self.worker.send(OpEvent::Progress {
            op_id: self.op_id,
            done_bytes: self.done_bytes,
            total_bytes: self.totals.bytes,
            done_files: self.done_files,
            total_files: self.totals.files,
            current: current.display().to_string(),
        });
    }
}

fn wide(p: &Path) -> Vec<u16> {
    p.as_os_str().encode_wide().chain(std::iter::once(0)).collect()
}

/// "file.txt" -> "file (2).txt", "file (3).txt", ...
pub fn auto_rename(dest: &Path) -> PathBuf {
    let stem = dest.file_stem().unwrap_or_default().to_string_lossy().into_owned();
    let ext = dest.extension().map(|e| e.to_string_lossy().into_owned());
    let parent = dest.parent().unwrap_or(Path::new(""));
    for n in 2..10_000 {
        let name = match &ext {
            Some(e) => format!("{stem} ({n}).{e}"),
            None => format!("{stem} ({n})"),
        };
        let candidate = parent.join(name);
        if !candidate.exists() {
            return candidate;
        }
    }
    dest.to_path_buf()
}

fn scan_totals(sources: &[PathBuf]) -> Totals {
    let mut t = Totals { bytes: 0, files: 0 };
    for s in sources {
        scan_one(s, &mut t);
    }
    t
}

fn scan_one(p: &Path, t: &mut Totals) {
    if let Ok(meta) = std::fs::symlink_metadata(p) {
        if meta.is_dir() {
            if let Ok(rd) = std::fs::read_dir(p) {
                for e in rd.flatten() {
                    scan_one(&e.path(), t);
                }
            }
        } else {
            t.bytes += meta.len();
            t.files += 1;
        }
    }
}

fn refresh_targets(op: &Operation) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = match op {
        Operation::Copy { sources, dest_dir } | Operation::Move { sources, dest_dir } => sources
            .iter()
            .filter_map(|s| s.parent().map(Path::to_path_buf))
            .chain(std::iter::once(dest_dir.clone()))
            .collect(),
        Operation::Delete { paths, .. } => paths
            .iter()
            .filter_map(|s| s.parent().map(Path::to_path_buf))
            .collect(),
        Operation::Rename { from, .. } => {
            from.parent().map(Path::to_path_buf).into_iter().collect()
        }
        Operation::NewFolder { path } | Operation::NewFile { path } => {
            path.parent().map(Path::to_path_buf).into_iter().collect()
        }
    };
    v.dedup();
    v
}

/// Send paths to the recycle bin via IFileOperation.
pub fn recycle_via_shell(paths: &[PathBuf]) -> Result<(), String> {
    unsafe {
        let op: IFileOperation = CoCreateInstance(&FileOperation, None, CLSCTX_ALL)
            .map_err(|e| e.message().to_string())?;
        op.SetOperationFlags(FOF_ALLOWUNDO | FOF_NOCONFIRMATION | FOF_NO_UI)
            .map_err(|e| e.message().to_string())?;
        op.SetOwnerWindow(HWND::default()).ok();
        for p in paths {
            let w = wide(p);
            let item: IShellItem =
                SHCreateItemFromParsingName(PCWSTR::from_raw(w.as_ptr()), None)
                    .map_err(|e| format!("{}: {}", p.display(), e.message()))?;
            op.DeleteItem(&item, None).map_err(|e| e.message().to_string())?;
        }
        op.PerformOperations().map_err(|e| e.message().to_string())?;
    }
    Ok(())
}
