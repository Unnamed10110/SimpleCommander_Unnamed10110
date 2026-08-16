//! Background file-operation queue. A capped worker pool runs non-overlapping
//! ops in parallel. Copies use `CopyFileExW`; recycle-bin deletes go through
//! `IFileOperation`. Each worker initializes COM on its own thread.

use crossbeam_channel::{unbounded, Receiver, Sender};
use parking_lot::{Condvar, Mutex};
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, AtomicUsize, Ordering};
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

const MAX_WORKERS: usize = 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OpOrigin {
    User,
    Undo,
    Redo,
}

#[derive(Clone, Debug)]
pub enum Operation {
    Copy { sources: Vec<PathBuf>, dest_dir: PathBuf },
    Move { sources: Vec<PathBuf>, dest_dir: PathBuf },
    Delete { paths: Vec<PathBuf>, recycle: bool },
    Rename { from: PathBuf, to: PathBuf },
    NewFolder { path: PathBuf },
    NewFile { path: PathBuf },
    RecycleRestore {
        parsing_names: Vec<String>,
        refresh: Vec<PathBuf>,
    },
    RecycleDelete { parsing_names: Vec<String> },
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
            Operation::RecycleRestore { parsing_names, .. } => {
                format!("Restore {} item(s)", parsing_names.len())
            }
            Operation::RecycleDelete { parsing_names } => {
                format!("Delete permanently {} item(s)", parsing_names.len())
            }
        }
    }
}

#[derive(Clone, Debug)]
pub enum OpEvent {
    Started {
        op_id: u64,
        origin: OpOrigin,
        label: String,
        total_bytes: u64,
        total_files: u64,
    },
    Progress {
        op_id: u64,
        done_bytes: u64,
        total_bytes: u64,
        done_files: u64,
        total_files: u64,
        current: String,
    },
    Conflict {
        op_id: u64,
        source: PathBuf,
        dest: PathBuf,
    },
    Done {
        op_id: u64,
        origin: OpOrigin,
        undo: Option<UndoAction>,
        refresh: Vec<PathBuf>,
        created: Vec<PathBuf>,
    },
    Failed {
        op_id: u64,
        origin: OpOrigin,
        error: String,
    },
    Cancelled { op_id: u64, origin: OpOrigin },
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

/// Fired after a successful copy/move/rename. Failures must not roll back the op.
pub type HookFn = Arc<dyn Fn(&str, &[PathBuf], &[PathBuf]) + Send + Sync>;

const PAUSE_NONE: u8 = 0;
const PAUSE_PAUSED: u8 = 1;
const PAUSE_CANCELLED: u8 = 2;

struct QueuedOp {
    id: u64,
    op: Operation,
    origin: OpOrigin,
}

struct Inner {
    pending: VecDeque<QueuedOp>,
    running: HashMap<u64, Vec<PathBuf>>,
    op_state: HashMap<u64, Arc<AtomicU8>>,
    conflict_tx: HashMap<u64, Sender<(ConflictResolution, bool)>>,
}

struct Shared {
    inner: Mutex<Inner>,
    cvar: Condvar,
    max_jobs: AtomicUsize,
    shutdown: AtomicBool,
    hooks: Mutex<Option<HookFn>>,
}

pub struct OpEngine {
    shared: Arc<Shared>,
    pub events: Receiver<OpEvent>,
    events_tx: Sender<OpEvent>,
    next_id: AtomicU64,
    notify: Arc<dyn Fn() + Send + Sync>,
}

impl OpEngine {
    pub fn new(notify: impl Fn() + Send + Sync + 'static) -> Self {
        let (event_tx, event_rx) = unbounded::<OpEvent>();
        let shared = Arc::new(Shared {
            inner: Mutex::new(Inner {
                pending: VecDeque::new(),
                running: HashMap::new(),
                op_state: HashMap::new(),
                conflict_tx: HashMap::new(),
            }),
            cvar: Condvar::new(),
            max_jobs: AtomicUsize::new(2),
            shutdown: AtomicBool::new(false),
            hooks: Mutex::new(None),
        });
        let notify: Arc<dyn Fn() + Send + Sync> = Arc::new(notify);
        for i in 0..MAX_WORKERS {
            let shared = shared.clone();
            let events = event_tx.clone();
            let notify = notify.clone();
            std::thread::Builder::new()
                .name(format!("sc-ops-{i}"))
                .spawn(move || worker_thread(shared, events, notify))
                .expect("spawn ops worker");
        }
        Self {
            shared,
            events: event_rx,
            events_tx: event_tx,
            next_id: AtomicU64::new(1),
            notify,
        }
    }

    pub fn set_max_jobs(&self, n: usize) {
        self.shared
            .max_jobs
            .store(n.clamp(1, MAX_WORKERS), Ordering::SeqCst);
        self.shared.cvar.notify_all();
    }

    pub fn set_hooks(&self, hooks: HookFn) {
        *self.shared.hooks.lock() = Some(hooks);
    }

    pub fn submit(&self, op: Operation) -> u64 {
        self.submit_origin(op, OpOrigin::User)
    }

    pub fn submit_origin(&self, op: Operation, origin: OpOrigin) -> u64 {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        {
            let mut g = self.shared.inner.lock();
            g.op_state
                .insert(id, Arc::new(AtomicU8::new(PAUSE_NONE)));
            g.pending.push_back(QueuedOp { id, op, origin });
        }
        self.shared.cvar.notify_all();
        id
    }

    pub fn pause(&self, op_id: u64) {
        let g = self.shared.inner.lock();
        if let Some(s) = g.op_state.get(&op_id) {
            let _ = s.compare_exchange(
                PAUSE_NONE,
                PAUSE_PAUSED,
                Ordering::SeqCst,
                Ordering::SeqCst,
            );
        }
    }

    pub fn resume(&self, op_id: u64) {
        let g = self.shared.inner.lock();
        if let Some(s) = g.op_state.get(&op_id) {
            let _ = s.compare_exchange(
                PAUSE_PAUSED,
                PAUSE_NONE,
                Ordering::SeqCst,
                Ordering::SeqCst,
            );
        }
    }

    pub fn cancel(&self, op_id: u64) {
        let g = self.shared.inner.lock();
        if let Some(s) = g.op_state.get(&op_id) {
            s.store(PAUSE_CANCELLED, Ordering::SeqCst);
        }
        self.shared.cvar.notify_all();
    }

    pub fn pause_all(&self) {
        let g = self.shared.inner.lock();
        for s in g.op_state.values() {
            let _ = s.compare_exchange(
                PAUSE_NONE,
                PAUSE_PAUSED,
                Ordering::SeqCst,
                Ordering::SeqCst,
            );
        }
    }

    pub fn resume_all(&self) {
        let g = self.shared.inner.lock();
        for s in g.op_state.values() {
            let _ = s.compare_exchange(
                PAUSE_PAUSED,
                PAUSE_NONE,
                Ordering::SeqCst,
                Ordering::SeqCst,
            );
        }
    }

    pub fn cancel_all(&self) {
        let g = self.shared.inner.lock();
        for s in g.op_state.values() {
            s.store(PAUSE_CANCELLED, Ordering::SeqCst);
        }
        self.shared.cvar.notify_all();
    }

    pub fn is_paused(&self, op_id: u64) -> bool {
        let g = self.shared.inner.lock();
        g.op_state
            .get(&op_id)
            .map(|s| s.load(Ordering::SeqCst) == PAUSE_PAUSED)
            .unwrap_or(false)
    }

    pub fn any_paused(&self) -> bool {
        let g = self.shared.inner.lock();
        g.op_state
            .values()
            .any(|s| s.load(Ordering::SeqCst) == PAUSE_PAUSED)
    }

    /// Answer a pending conflict prompt for `op_id`.
    pub fn resolve_conflict(&self, op_id: u64, res: ConflictResolution, apply_to_all: bool) {
        let tx = {
            let mut g = self.shared.inner.lock();
            g.conflict_tx.remove(&op_id)
        };
        if let Some(tx) = tx {
            let _ = tx.send((res, apply_to_all));
        }
    }
}

impl Drop for OpEngine {
    fn drop(&mut self) {
        self.shared.shutdown.store(true, Ordering::SeqCst);
        self.shared.cvar.notify_all();
        let _ = self.events_tx;
        let _ = self.notify;
    }
}

fn worker_thread(shared: Arc<Shared>, events: Sender<OpEvent>, notify: Arc<dyn Fn() + Send + Sync>) {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
    }
    loop {
        let job = {
            let mut g = shared.inner.lock();
            loop {
                if shared.shutdown.load(Ordering::SeqCst) {
                    drop(g);
                    unsafe { CoUninitialize() };
                    return;
                }
                // Drop cancelled queued ops without running them.
                let mut i = 0;
                while i < g.pending.len() {
                    let id = g.pending[i].id;
                    let cancelled = g
                        .op_state
                        .get(&id)
                        .map(|s| s.load(Ordering::SeqCst) == PAUSE_CANCELLED)
                        .unwrap_or(false);
                    if cancelled {
                        let q = g.pending.remove(i).unwrap();
                        g.op_state.remove(&q.id);
                        let _ = events.send(OpEvent::Cancelled {
                            op_id: q.id,
                            origin: q.origin,
                        });
                        notify();
                    } else {
                        i += 1;
                    }
                }
                let cap = shared.max_jobs.load(Ordering::SeqCst).clamp(1, MAX_WORKERS);
                if g.running.len() < cap {
                    if let Some(idx) = g.pending.iter().position(|q| {
                        !running_overlap(&g.running, &touch_paths(&q.op))
                    }) {
                        let q = g.pending.remove(idx).unwrap();
                        g.running.insert(q.id, touch_paths(&q.op));
                        break q;
                    }
                }
                shared.cvar.wait(&mut g);
            }
        };

        let state = {
            let g = shared.inner.lock();
            g.op_state
                .get(&job.id)
                .cloned()
                .unwrap_or_else(|| Arc::new(AtomicU8::new(PAUSE_NONE)))
        };

        let mut worker = Worker {
            events: events.clone(),
            shared: shared.clone(),
            state,
            notify: notify.clone(),
            origin: job.origin,
        };
        worker.run(job.id, job.op);

        {
            let mut g = shared.inner.lock();
            g.running.remove(&job.id);
            g.op_state.remove(&job.id);
            g.conflict_tx.remove(&job.id);
        }
        shared.cvar.notify_all();
    }
}

fn running_overlap(running: &HashMap<u64, Vec<PathBuf>>, paths: &[PathBuf]) -> bool {
    running
        .values()
        .any(|rp| rp.iter().any(|a| paths.iter().any(|b| paths_overlap(a, b))))
}

pub fn paths_overlap(a: &Path, b: &Path) -> bool {
    a.starts_with(b) || b.starts_with(a)
}

fn touch_paths(op: &Operation) -> Vec<PathBuf> {
    match op {
        Operation::Copy { sources, dest_dir } | Operation::Move { sources, dest_dir } => {
            let mut v = sources.clone();
            v.push(dest_dir.clone());
            v
        }
        Operation::Delete { paths, .. } => paths.clone(),
        Operation::Rename { from, to } => vec![from.clone(), to.clone()],
        Operation::NewFolder { path } | Operation::NewFile { path } => vec![path.clone()],
        Operation::RecycleRestore { refresh, .. } => refresh.clone(),
        Operation::RecycleDelete { .. } => vec![PathBuf::from(r"recycle:\")],
    }
}

struct Worker {
    events: Sender<OpEvent>,
    shared: Arc<Shared>,
    state: Arc<AtomicU8>,
    notify: Arc<dyn Fn() + Send + Sync>,
    origin: OpOrigin,
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

    fn pause_point(&self) -> bool {
        loop {
            match self.state.load(Ordering::SeqCst) {
                PAUSE_PAUSED => std::thread::sleep(std::time::Duration::from_millis(50)),
                PAUSE_CANCELLED => return false,
                _ => return true,
            }
        }
    }

    fn fire_hook(&self, event: &str, sources: &[PathBuf], dests: &[PathBuf]) {
        if let Some(h) = self.shared.hooks.lock().clone() {
            h(event, sources, dests);
        }
    }

    fn run(&mut self, op_id: u64, op: Operation) {
        let refresh = refresh_targets(&op);
        let origin = self.origin;
        let result = match op {
            Operation::Copy { sources, dest_dir } => {
                self.copy_or_move(op_id, sources, dest_dir, false)
            }
            Operation::Move { sources, dest_dir } => {
                self.copy_or_move(op_id, sources, dest_dir, true)
            }
            Operation::Delete { paths, recycle } => self.delete(op_id, paths, recycle),
            Operation::Rename { from, to } => self.rename(op_id, from, to),
            Operation::NewFolder { path } => {
                self.send(OpEvent::Started {
                    op_id,
                    origin,
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
                    origin,
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
            Operation::RecycleRestore {
                parsing_names,
                refresh: _,
            } => {
                self.send(OpEvent::Started {
                    op_id,
                    origin,
                    label: format!("Restore {} item(s)", parsing_names.len()),
                    total_bytes: 0,
                    total_files: parsing_names.len() as u64,
                });
                sc_shell::recycle::restore_items(&parsing_names).map(|_| None)
            }
            Operation::RecycleDelete { parsing_names } => {
                self.send(OpEvent::Started {
                    op_id,
                    origin,
                    label: format!("Delete permanently {} item(s)", parsing_names.len()),
                    total_bytes: 0,
                    total_files: parsing_names.len() as u64,
                });
                sc_shell::recycle::delete_permanent(&parsing_names).map(|_| None)
            }
        };
        match result {
            Ok(undo) => {
                let created = created_dests(&undo);
                self.send(OpEvent::Done {
                    op_id,
                    origin,
                    undo,
                    refresh,
                    created,
                });
            }
            Err(e) if e == "__cancelled__" => self.send(OpEvent::Cancelled { op_id, origin }),
            Err(e) => self.send(OpEvent::Failed {
                op_id,
                origin,
                error: e,
            }),
        }
    }

    fn rename(&self, op_id: u64, from: PathBuf, to: PathBuf) -> Result<Option<UndoAction>, String> {
        self.send(OpEvent::Started {
            op_id,
            origin: self.origin,
            label: format!(
                "Rename to {}",
                to.file_name().unwrap_or_default().to_string_lossy()
            ),
            total_bytes: 0,
            total_files: 1,
        });
        std::fs::rename(&from, &to).map_err(|e| e.to_string())?;
        self.fire_hook("after-rename", &[from.clone()], &[to.clone()]);
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
            origin: self.origin,
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
        let dests: Vec<PathBuf> = if is_move {
            ctx.moved_pairs.iter().map(|(_, to)| to.clone()).collect()
        } else {
            ctx.created.clone()
        };
        let event = if is_move { "after-move" } else { "after-copy" };
        ctx.worker.fire_hook(event, &sources, &dests);
        Ok(if is_move {
            Some(UndoAction::MoveBack {
                pairs: ctx.moved_pairs,
            })
        } else if ctx.created.is_empty() {
            None
        } else {
            Some(UndoAction::DeletePaths(ctx.created))
        })
    }

    fn delete(
        &self,
        op_id: u64,
        paths: Vec<PathBuf>,
        recycle: bool,
    ) -> Result<Option<UndoAction>, String> {
        self.send(OpEvent::Started {
            op_id,
            origin: self.origin,
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

    fn ask_conflict(&self, op_id: u64, source: &Path, dest: &Path) -> (ConflictResolution, bool) {
        let (tx, rx) = unbounded();
        {
            let mut g = self.shared.inner.lock();
            g.conflict_tx.insert(op_id, tx);
        }
        self.send(OpEvent::Conflict {
            op_id,
            source: source.to_path_buf(),
            dest: dest.to_path_buf(),
        });
        rx.recv()
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
    created: Vec<PathBuf>,
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
                    PAUSE_CANCELLED => {
                        return windows::Win32::Storage::FileSystem::COPYPROGRESSROUTINE_PROGRESS(1)
                    }
                    _ => {
                        return windows::Win32::Storage::FileSystem::COPYPROGRESSROUTINE_PROGRESS(0)
                    }
                }
            }
        }
        let ctx = CbCtx {
            state: self.worker.state.clone(),
        };
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
    p.as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

/// "file.txt" -> "file (2).txt", "file (3).txt", ...
pub fn auto_rename(dest: &Path) -> PathBuf {
    let stem = dest
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
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

fn created_dests(undo: &Option<UndoAction>) -> Vec<PathBuf> {
    match undo {
        Some(UndoAction::DeletePaths(paths)) => paths.clone(),
        Some(UndoAction::MoveBack { pairs }) => pairs.iter().map(|(_, to)| to.clone()).collect(),
        Some(UndoAction::RenameBack { from, .. }) => vec![from.clone()],
        None => Vec::new(),
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
        Operation::Rename { from, .. } => from.parent().map(Path::to_path_buf).into_iter().collect(),
        Operation::NewFolder { path } | Operation::NewFile { path } => {
            path.parent().map(Path::to_path_buf).into_iter().collect()
        }
        Operation::RecycleRestore { refresh, .. } => {
            let mut r = refresh.clone();
            r.push(PathBuf::from(r"recycle:\"));
            r
        }
        Operation::RecycleDelete { .. } => vec![PathBuf::from(r"recycle:\")],
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
            op.DeleteItem(&item, None)
                .map_err(|e| e.message().to_string())?;
        }
        op.PerformOperations()
            .map_err(|e| e.message().to_string())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overlap_detects_nested_paths() {
        let a = PathBuf::from(r"C:\foo");
        let b = PathBuf::from(r"C:\foo\bar");
        assert!(paths_overlap(&a, &b));
        assert!(!paths_overlap(&a, &PathBuf::from(r"D:\foo")));
    }
}
