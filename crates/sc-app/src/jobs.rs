//! Worker-thread job engine. The UI thread never touches the filesystem:
//! it submits jobs here and consumes ready results each frame. Results for
//! stale generations are dropped by the receiver.

use crossbeam_channel::{unbounded, Receiver, Sender};
use sc_core::sort::{build_view, SortSpec};
use sc_core::FsEntry;
use sc_index::search::IndexService;
use sc_shell::icons::IconRgba;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

/// Identifies one listing request (pane/tab/generation).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ListingToken {
    pub pane: usize,
    pub tab_uid: u64,
    pub generation: u64,
}

pub enum Job {
    ReadDir { token: ListingToken, path: PathBuf, flatten: bool },
    BuildView {
        token: ListingToken,
        entries: Arc<Vec<FsEntry>>,
        spec: SortSpec,
        filter: String,
        show_hidden: bool,
    },
    IconExt { key: String, ext: String, is_dir: bool },
    IconPath { key: String, path: PathBuf },
    DirSize { path: PathBuf },
    SearchNames {
        query_id: u64,
        query: String,
        max: usize,
        scope: Option<PathBuf>,
    },
    ContentSearch {
        query_id: u64,
        scope: PathBuf,
        needle: String,
        max_results: usize,
        max_file_size: u64,
    },
    Preview { path: PathBuf, generation: u64 },
    ColumnValue { plugin: usize, path: PathBuf },
    Checksum { path: PathBuf },
    /// Subdirectory names only (for the tree sidebar).
    ListDirs { path: PathBuf },
    CompareFolders {
        query_id: u64,
        left: PathBuf,
        right: PathBuf,
        recursive: bool,
    },
}

pub enum PreviewContent {
    Image { size: [usize; 2], rgba: Vec<u8> },
    Text(String),
    Info(String),
    Hex { file_size: u64, bytes: Vec<u8> },
    Audio {
        path: PathBuf,
        lines: Vec<(String, String)>,
        duration_secs: Option<f64>,
        cover: Option<([usize; 2], Vec<u8>)>,
    },
    Web {
        url: String,
        fallback_text: Option<String>,
    },
}

pub enum UiMsg {
    Batch {
        token: ListingToken,
        entries: Vec<FsEntry>,
        done: bool,
        error: Option<String>,
    },
    View { token: ListingToken, view: Vec<u32> },
    Icon { key: String, image: Option<IconRgba> },
    DirSize { path: PathBuf, size: u64 },
    SearchResults { query_id: u64, results: Vec<(PathBuf, bool)>, done: bool },
    Preview { path: PathBuf, generation: u64, content: PreviewContent },
    ColumnValue { plugin: usize, path: PathBuf, value: Option<String> },
    Checksum { path: PathBuf, value: Option<String> },
    DirChanged { pane: usize, tab_uid: u64 },
    DirsListed { path: PathBuf, dirs: Vec<String> },
    RecycleMeta { items: Vec<sc_shell::recycle::RecycleItem> },
    CompareResult {
        query_id: u64,
        rows: Vec<crate::compare::CompareRow>,
    },
}

pub struct JobEngine {
    jobs: Sender<Job>,
    pub results: Receiver<UiMsg>,
    pub results_tx: Sender<UiMsg>,
    /// Latest search query id; workers drop results for stale queries.
    pub search_epoch: Arc<AtomicU64>,
    pub index: Arc<IndexService>,
    pub plugins: Arc<parking_lot::RwLock<sc_plugins::host::PluginHost>>,
}

impl JobEngine {
    pub fn new(
        ctx: egui::Context,
        plugins: Arc<parking_lot::RwLock<sc_plugins::host::PluginHost>>,
        index_enabled: bool,
    ) -> Self {
        let (job_tx, job_rx) = unbounded::<Job>();
        let (res_tx, res_rx) = unbounded::<UiMsg>();
        let search_epoch = Arc::new(AtomicU64::new(0));

        let idx_ctx = ctx.clone();
        let index = IndexService::start(index_enabled, move || idx_ctx.request_repaint());

        let workers = std::thread::available_parallelism()
            .map(|n| n.get().clamp(2, 6))
            .unwrap_or(4);
        for i in 0..workers {
            let rx = job_rx.clone();
            let tx = res_tx.clone();
            let ctx = ctx.clone();
            let epoch = search_epoch.clone();
            let index = index.clone();
            let plugins = plugins.clone();
            std::thread::Builder::new()
                .name(format!("sc-worker-{i}"))
                .spawn(move || {
                    // COM for SHGetFileInfoW etc.
                    unsafe {
                        use windows::Win32::System::Com::{
                            CoInitializeEx, COINIT_MULTITHREADED,
                        };
                        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
                    }
                    while let Ok(job) = rx.recv() {
                        run_job(job, &tx, &ctx, &epoch, &index, &plugins);
                    }
                })
                .expect("spawn worker");
        }

        Self {
            jobs: job_tx,
            results: res_rx,
            results_tx: res_tx,
            search_epoch,
            index,
            plugins,
        }
    }

    pub fn submit(&self, job: Job) {
        let _ = self.jobs.send(job);
    }

    /// Bump the search epoch (cancels in-flight searches) and return the new id.
    pub fn new_search(&self) -> u64 {
        self.search_epoch.fetch_add(1, Ordering::SeqCst) + 1
    }
}

fn run_job(
    job: Job,
    tx: &Sender<UiMsg>,
    ctx: &egui::Context,
    epoch: &AtomicU64,
    index: &IndexService,
    plugins: &parking_lot::RwLock<sc_plugins::host::PluginHost>,
) {
    let send = |msg: UiMsg| {
        let _ = tx.send(msg);
        ctx.request_repaint();
    };
    match job {
        Job::ReadDir { token, path, flatten } => {
            if sc_shell::recycle::is_recycle_path(&path) {
                match sc_shell::recycle::list_recycle() {
                    Ok(items) => {
                        send(UiMsg::RecycleMeta { items: items.clone() });
                        let entries: Vec<FsEntry> = items.iter().map(|i| i.to_entry()).collect();
                        send(UiMsg::Batch {
                            token,
                            entries,
                            done: true,
                            error: None,
                        });
                    }
                    Err(e) => send(UiMsg::Batch {
                        token,
                        entries: Vec::new(),
                        done: true,
                        error: Some(e),
                    }),
                }
                return;
            }
            if let Some(listing) = crate::vfs::zip_listing(&path) {
                match listing {
                    Ok(entries) => {
                        send(UiMsg::Batch { token, entries, done: true, error: None });
                    }
                    Err(e) => send(UiMsg::Batch {
                        token,
                        entries: Vec::new(),
                        done: true,
                        error: Some(e),
                    }),
                }
                return;
            }
            let result = if flatten {
                let mut err = None;
                let r = sc_shell::enumerate::enumerate_tree(&path, &mut |rel, batch| {
                    let mapped: Vec<FsEntry> = batch
                        .into_iter()
                        .map(|mut e| {
                            if !rel.as_os_str().is_empty() {
                                e.name = format!("{}\\{}", rel.display(), e.name);
                            }
                            e
                        })
                        .collect();
                    send(UiMsg::Batch { token, entries: mapped, done: false, error: None });
                    true
                });
                if let Err(e) = r {
                    err = Some(e);
                }
                err
            } else {
                sc_shell::enumerate::enumerate_dir(&path, |batch| {
                    send(UiMsg::Batch { token, entries: batch, done: false, error: None });
                    true
                })
                .err()
            };
            send(UiMsg::Batch { token, entries: Vec::new(), done: true, error: result });
        }
        Job::BuildView { token, entries, spec, filter, show_hidden } => {
            let view = build_view(&entries, spec, &filter, show_hidden);
            send(UiMsg::View { token, view });
        }
        Job::IconExt { key, ext, is_dir } => {
            let image = sc_shell::icons::icon_for_extension(&ext, is_dir);
            send(UiMsg::Icon { key, image });
        }
        Job::IconPath { key, path } => {
            let image = sc_shell::icons::icon_for_path(&path);
            send(UiMsg::Icon { key, image });
        }
        Job::DirSize { path } => {
            let size = sc_shell::enumerate::dir_size(&path, &|| false);
            send(UiMsg::DirSize { path, size });
        }
        Job::SearchNames { query_id, query, max, scope } => {
            if epoch.load(Ordering::SeqCst) != query_id {
                return;
            }
            let results = index.search_names(
                &query,
                max.max(1),
                scope.as_deref(),
                &|| epoch.load(Ordering::SeqCst) != query_id,
            );
            if epoch.load(Ordering::SeqCst) == query_id {
                send(UiMsg::SearchResults { query_id, results, done: true });
            }
        }
        Job::ContentSearch { query_id, scope, needle, max_results, max_file_size } => {
            if epoch.load(Ordering::SeqCst) != query_id {
                return;
            }
            let hits = sc_index::search::content_search(
                &scope,
                &needle,
                max_results.max(1),
                max_file_size.max(1),
                &|| epoch.load(Ordering::SeqCst) != query_id,
            );
            if epoch.load(Ordering::SeqCst) == query_id {
                let results = hits.into_iter().map(|p| (p, false)).collect();
                send(UiMsg::SearchResults { query_id, results, done: true });
            }
        }
        Job::Preview { path, generation } => {
            let content = load_preview(&path, plugins);
            send(UiMsg::Preview { path, generation, content });
        }
        Job::ColumnValue { plugin, path } => {
            let value = plugins
                .read()
                .column_value(plugin, &path.to_string_lossy());
            send(UiMsg::ColumnValue { plugin, path, value });
        }
        Job::Checksum { path } => {
            let value = sha256_file(&path);
            send(UiMsg::Checksum { path, value });
        }
        Job::ListDirs { path } => {
            let mut dirs: Vec<String> = Vec::new();
            let _ = sc_shell::enumerate::enumerate_dir(&path, |batch| {
                for e in batch {
                    if e.is_dir() && !e.is_hidden() {
                        dirs.push(e.name);
                    }
                }
                dirs.len() < 2000
            });
            dirs.sort_by(|a, b| sc_core::sort::natural_cmp(a, b));
            send(UiMsg::DirsListed { path, dirs });
        }
        Job::CompareFolders {
            query_id,
            left,
            right,
            recursive,
        } => {
            let rows = crate::compare::compare_folders(&left, &right, recursive);
            send(UiMsg::CompareResult { query_id, rows });
        }
    }
}

const TEXT_PREVIEW_EXTS: &[&str] = &[
    "txt", "md", "rs", "toml", "json", "xml", "css", "js", "ts", "py", "c",
    "cpp", "h", "hpp", "cs", "java", "go", "rb", "sh", "ps1", "bat", "cmd", "ini", "cfg",
    "conf", "log", "yml", "yaml", "sql", "csv", "tsv", "gitignore", "lock",
];
const IMAGE_PREVIEW_EXTS: &[&str] =
    &["png", "jpg", "jpeg", "gif", "bmp", "webp", "ico", "tif", "tiff", "tga"];
const AUDIO_PREVIEW_EXTS: &[&str] = &[
    "mp3", "wav", "flac", "ogg", "oga", "m4a", "aac", "wma", "opus", "aiff", "aif", "ape",
];
const VIDEO_PREVIEW_EXTS: &[&str] =
    &["mp4", "webm", "mkv", "avi", "mov", "wmv", "m4v", "mpg", "mpeg"];
const HTML_PREVIEW_EXTS: &[&str] = &["html", "htm", "xhtml"];
const HEX_MAX: usize = 128 * 1024;

fn load_preview(
    path: &std::path::Path,
    plugins: &parking_lot::RwLock<sc_plugins::host::PluginHost>,
) -> PreviewContent {
    if let Some((kind, body)) = plugins.read().preview(path) {
        return match kind.as_str() {
            "text" => PreviewContent::Text(body),
            "hex" => {
                let bytes = body.into_bytes();
                PreviewContent::Hex {
                    file_size: bytes.len() as u64,
                    bytes,
                }
            }
            _ => PreviewContent::Info(body),
        };
    }
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    if IMAGE_PREVIEW_EXTS.contains(&ext.as_str()) {
        match image::open(path) {
            Ok(img) => {
                let img = if img.width() > 2048 || img.height() > 2048 {
                    img.thumbnail(2048, 2048)
                } else {
                    img
                };
                let rgba = img.to_rgba8();
                let (w, h) = (rgba.width() as usize, rgba.height() as usize);
                return PreviewContent::Image { size: [w, h], rgba: rgba.into_raw() };
            }
            Err(e) => return PreviewContent::Info(format!("Cannot decode image: {e}")),
        }
    }
    if AUDIO_PREVIEW_EXTS.contains(&ext.as_str()) {
        return load_audio_preview(path);
    }
    if ext == "pdf"
        || HTML_PREVIEW_EXTS.contains(&ext.as_str())
        || VIDEO_PREVIEW_EXTS.contains(&ext.as_str())
    {
        let url = match file_url(path) {
            Some(u) => u,
            None => return PreviewContent::Info("Cannot build a file URL for this path.".into()),
        };
        let fallback_text = if HTML_PREVIEW_EXTS.contains(&ext.as_str()) {
            read_text_head(path, 128 * 1024)
        } else {
            None
        };
        return PreviewContent::Web { url, fallback_text };
    }
    if TEXT_PREVIEW_EXTS.contains(&ext.as_str()) || ext.is_empty() {
        match read_text_head(path, 128 * 1024) {
            Some(text) => return PreviewContent::Text(text),
            None => {}
        }
    }
    load_hex_preview(path)
}

fn load_audio_preview(path: &std::path::Path) -> PreviewContent {
    let mut lines = Vec::new();
    let mut duration_secs = None;
    let mut cover = None;
    if let Ok(tagged) = lofty::read_from_path(path) {
        let props = tagged.properties();
        duration_secs = Some(props.duration().as_secs_f64());
        if let Some(sr) = props.sample_rate().filter(|v| *v > 0) {
            lines.push(("Sample rate".into(), format!("{sr} Hz")));
        }
        if let Some(ch) = props.channels().filter(|v| *v > 0) {
            lines.push(("Channels".into(), ch.to_string()));
        }
        if let Some(br) = props.audio_bitrate().filter(|b| *b > 0) {
            lines.push(("Bitrate".into(), format!("{br} kbps")));
        }
        use lofty::prelude::*;
        if let Some(tag) = tagged.primary_tag().or_else(|| tagged.first_tag()) {
            if let Some(v) = tag.title() {
                lines.insert(0, ("Title".into(), v.to_string()));
            }
            if let Some(v) = tag.artist() {
                lines.push(("Artist".into(), v.to_string()));
            }
            if let Some(v) = tag.album() {
                lines.push(("Album".into(), v.to_string()));
            }
            if let Some(v) = tag.year() {
                lines.push(("Year".into(), v.to_string()));
            }
            if let Some(v) = tag.genre() {
                lines.push(("Genre".into(), v.to_string()));
            }
            if let Some(pic) = tag.pictures().first() {
                if let Ok(img) = image::load_from_memory(pic.data()) {
                    let img = img.thumbnail(512, 512).to_rgba8();
                    let (w, h) = (img.width() as usize, img.height() as usize);
                    cover = Some(([w, h], img.into_raw()));
                }
            }
        }
    }
    if lines.is_empty() {
        if let Some(name) = path.file_name() {
            lines.push(("File".into(), name.to_string_lossy().into_owned()));
        }
    }
    PreviewContent::Audio {
        path: path.to_path_buf(),
        lines,
        duration_secs,
        cover,
    }
}

fn load_hex_preview(path: &std::path::Path) -> PreviewContent {
    use std::io::Read;
    match std::fs::File::open(path) {
        Ok(mut f) => {
            let file_size = f.metadata().map(|m| m.len()).unwrap_or(0);
            if file_size == 0 {
                return PreviewContent::Info("Empty file".into());
            }
            let take = HEX_MAX.min(file_size as usize);
            let mut bytes = vec![0u8; take];
            match f.read(&mut bytes) {
                Ok(n) => {
                    bytes.truncate(n);
                    PreviewContent::Hex { file_size, bytes }
                }
                Err(e) => PreviewContent::Info(format!("Cannot read file: {e}")),
            }
        }
        Err(e) => PreviewContent::Info(format!("Cannot open file: {e}")),
    }
}

fn file_url(path: &std::path::Path) -> Option<String> {
    let abs = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let stripped = strip_verbatim(&abs);
    url::Url::from_file_path(&stripped)
        .ok()
        .map(|u| u.to_string())
}

fn strip_verbatim(path: &std::path::Path) -> std::path::PathBuf {
    let s = path.to_string_lossy();
    if let Some(rest) = s.strip_prefix(r"\\?\") {
        std::path::PathBuf::from(rest)
    } else {
        path.to_path_buf()
    }
}

fn read_text_head(path: &std::path::Path, max: usize) -> Option<String> {
    use std::io::Read;
    let mut f = std::fs::File::open(path).ok()?;
    let mut buf = vec![0u8; max];
    let n = f.read(&mut buf).ok()?;
    buf.truncate(n);
    // Reject if it looks binary (NUL bytes in the head).
    if buf.iter().take(4096).any(|&b| b == 0) {
        return None;
    }
    Some(String::from_utf8_lossy(&buf).into_owned())
}

fn sha256_file(path: &std::path::Path) -> Option<String> {
    use sha2::{Digest, Sha256};
    use std::io::Read;
    let mut f = std::fs::File::open(path).ok()?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 256 * 1024];
    loop {
        let n = f.read(&mut buf).ok()?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Some(format!("{:x}", hasher.finalize()))
}
