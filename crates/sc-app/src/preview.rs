//! Preview window: images, text, hex, audio (tags + playback), and a WebView2
//! child window for PDF / HTML / video.

use crate::app::{PreviewState, ScApp};
use dpi::{PhysicalPosition, PhysicalSize};
use egui::{Align2, RichText, Sense, Ui};
use parking_lot::Mutex;
use raw_window_handle::{
    DisplayHandle, HandleError, HasDisplayHandle, HasWindowHandle, RawDisplayHandle, RawWindowHandle,
    Win32WindowHandle, WindowHandle, WindowsDisplayHandle,
};
use std::fs::File;
use std::io::BufReader;
use std::num::NonZeroIsize;
use std::path::PathBuf;
use std::sync::mpsc::{self, Sender};
use std::sync::Arc;
use wry::{Rect, WebView, WebViewBuilder};

const HEX_COLS: usize = 16;

#[derive(Clone)]
pub struct HexPreview {
    pub file_size: u64,
    pub bytes: Vec<u8>,
}

#[derive(Clone)]
pub struct AudioPreview {
    pub path: PathBuf,
    pub lines: Vec<(String, String)>,
    pub duration_secs: Option<f64>,
}

enum AudioCmd {
    Play(PathBuf),
    Pause,
    Resume,
    Stop,
    Shutdown,
}

pub struct AudioCtl {
    tx: Option<Sender<AudioCmd>>,
    playing: Arc<Mutex<bool>>,
    paused: Arc<Mutex<bool>>,
    error: Arc<Mutex<Option<String>>>,
}

impl Default for AudioCtl {
    fn default() -> Self {
        Self {
            tx: None,
            playing: Arc::new(Mutex::new(false)),
            paused: Arc::new(Mutex::new(false)),
            error: Arc::new(Mutex::new(None)),
        }
    }
}

impl AudioCtl {
    fn ensure_thread(&mut self) {
        if self.tx.is_some() {
            return;
        }
        let (tx, rx) = mpsc::channel();
        let playing = Arc::clone(&self.playing);
        let paused = Arc::clone(&self.paused);
        let error = Arc::clone(&self.error);
        std::thread::Builder::new()
            .name("sc-audio".into())
            .spawn(move || {
                let Ok((_stream, handle)) = rodio::OutputStream::try_default() else {
                    *error.lock() = Some("No audio output device".into());
                    return;
                };
                let Ok(sink) = rodio::Sink::try_new(&handle) else {
                    *error.lock() = Some("Cannot open audio sink".into());
                    return;
                };
                while let Ok(cmd) = rx.recv() {
                    match cmd {
                        AudioCmd::Play(path) => {
                            sink.stop();
                            match File::open(&path) {
                                Ok(f) => match rodio::Decoder::new(BufReader::new(f)) {
                                    Ok(dec) => {
                                        *error.lock() = None;
                                        sink.append(dec);
                                        sink.play();
                                        *playing.lock() = true;
                                        *paused.lock() = false;
                                    }
                                    Err(e) => {
                                        *error.lock() = Some(format!("Cannot decode audio: {e}"));
                                        *playing.lock() = false;
                                    }
                                },
                                Err(e) => {
                                    *error.lock() = Some(format!("Cannot open audio: {e}"));
                                    *playing.lock() = false;
                                }
                            }
                        }
                        AudioCmd::Pause => {
                            sink.pause();
                            *playing.lock() = false;
                            *paused.lock() = true;
                        }
                        AudioCmd::Resume => {
                            sink.play();
                            *playing.lock() = true;
                            *paused.lock() = false;
                        }
                        AudioCmd::Stop => {
                            sink.stop();
                            *playing.lock() = false;
                            *paused.lock() = false;
                        }
                        AudioCmd::Shutdown => break,
                    }
                }
                sink.stop();
            })
            .ok();
        self.tx = Some(tx);
    }

    pub fn play(&mut self, path: PathBuf) {
        self.ensure_thread();
        if let Some(tx) = &self.tx {
            let _ = tx.send(AudioCmd::Play(path));
        }
        *self.playing.lock() = true;
        *self.paused.lock() = false;
    }

    pub fn pause(&mut self) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(AudioCmd::Pause);
        }
        *self.playing.lock() = false;
        *self.paused.lock() = true;
    }

    pub fn resume(&mut self) {
        self.ensure_thread();
        if let Some(tx) = &self.tx {
            let _ = tx.send(AudioCmd::Resume);
        }
        *self.playing.lock() = true;
        *self.paused.lock() = false;
    }

    pub fn stop(&mut self) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(AudioCmd::Stop);
        }
        *self.playing.lock() = false;
        *self.paused.lock() = false;
    }

    pub fn is_playing(&self) -> bool {
        *self.playing.lock()
    }

    pub fn is_paused(&self) -> bool {
        *self.paused.lock()
    }

    pub fn last_error(&self) -> Option<String> {
        self.error.lock().clone()
    }

    pub fn shutdown(&mut self) {
        if let Some(tx) = self.tx.take() {
            let _ = tx.send(AudioCmd::Shutdown);
        }
    }
}

impl Drop for AudioCtl {
    fn drop(&mut self) {
        self.shutdown();
    }
}

pub struct WebEmbed {
    view: WebView,
    url: String,
    visible: bool,
}

struct ParentHwnd {
    hwnd: NonZeroIsize,
}

impl HasWindowHandle for ParentHwnd {
    fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
        let handle = Win32WindowHandle::new(self.hwnd);
        unsafe { Ok(WindowHandle::borrow_raw(RawWindowHandle::Win32(handle))) }
    }
}

impl HasDisplayHandle for ParentHwnd {
    fn display_handle(&self) -> Result<DisplayHandle<'_>, HandleError> {
        unsafe {
            Ok(DisplayHandle::borrow_raw(RawDisplayHandle::Windows(
                WindowsDisplayHandle::new(),
            )))
        }
    }
}

pub fn capture_parent(frame: &eframe::Frame) -> Option<isize> {
    let handle = frame.window_handle().ok()?;
    match handle.as_raw() {
        RawWindowHandle::Win32(h) => Some(h.hwnd.get()),
        _ => None,
    }
}

pub fn shutdown(preview: &mut PreviewState) {
    preview.audio_ctl.stop();
    preview.audio_ctl.shutdown();
    destroy_web(preview);
}

pub fn close(preview: &mut PreviewState) {
    preview.enabled = false;
    preview.audio_ctl.stop();
    destroy_web(preview);
}

pub fn destroy_web(preview: &mut PreviewState) {
    if let Some(web) = preview.webview.take() {
        let _ = web.view.set_visible(false);
        let _ = web.view.set_bounds(Rect {
            position: PhysicalPosition::new(-32000.0, -32000.0).into(),
            size: PhysicalSize::new(1.0, 1.0).into(),
        });
        let _ = web.view.focus_parent();
    }
    preview.embed_rect = None;
}

fn sync_web(preview: &mut PreviewState, ppp: f32) {
    let want_url = preview.web_url.clone();
    let enabled = preview.enabled && want_url.is_some();
    let rect = preview.embed_rect;
    let hwnd = preview.parent_hwnd;

    if !enabled {
        destroy_web(preview);
        return;
    }
    let Some(url) = want_url else {
        destroy_web(preview);
        return;
    };
    let Some(rect) = rect else {
        destroy_web(preview);
        return;
    };
    if rect.width() < 8.0 || rect.height() < 8.0 {
        destroy_web(preview);
        return;
    }
    let Some(hwnd) = hwnd.and_then(NonZeroIsize::new) else {
        preview.webview_error = Some("Cannot attach preview (no window handle)".into());
        return;
    };

    if preview.webview.is_none() {
        match create_webview(hwnd, &url, rect, ppp) {
            Ok(web) => {
                preview.webview = Some(web);
                preview.webview_error = None;
            }
            Err(e) => {
                preview.webview_error = Some(format!(
                    "WebView2 is required for PDF, HTML and video preview.\n{e}"
                ));
            }
        }
        return;
    }

    if let Some(web) = preview.webview.as_mut() {
        if web.url != url {
            if let Err(e) = web.view.load_url(&url) {
                preview.webview_error = Some(format!("Cannot load preview: {e}"));
            } else {
                web.url = url;
                preview.webview_error = None;
            }
        }
        let bounds = egui_rect_to_wry(rect, ppp);
        let _ = web.view.set_bounds(bounds);
        if !web.visible {
            let _ = web.view.set_visible(true);
            web.visible = true;
        }
    }
}

fn create_webview(hwnd: NonZeroIsize, url: &str, rect: egui::Rect, ppp: f32) -> Result<WebEmbed, String> {
    let parent = ParentHwnd { hwnd };
    let bounds = egui_rect_to_wry(rect, ppp);
    let view = WebViewBuilder::new()
        .with_url(url)
        .with_bounds(bounds)
        .with_visible(true)
        .with_focused(false)
        .with_devtools(false)
        .with_background_color((0, 0, 0, 255))
        .build_as_child(&parent)
        .map_err(|e| e.to_string())?;
    Ok(WebEmbed {
        view,
        url: url.to_string(),
        visible: true,
    })
}

fn egui_rect_to_wry(rect: egui::Rect, ppp: f32) -> Rect {
    Rect {
        position: PhysicalPosition::new(rect.min.x * ppp, rect.min.y * ppp).into(),
        size: PhysicalSize::new(rect.width() * ppp, rect.height() * ppp).into(),
    }
}

pub fn draw(app: &mut ScApp, ctx: &egui::Context) {
    if !app.preview.enabled {
        destroy_web(&mut app.preview);
        return;
    }
    if poll_webview_keys(&mut app.preview) {
        close(&mut app.preview);
        return;
    }
    let mut open = true;
    egui::Window::new("Preview")
        .open(&mut open)
        .pivot(Align2::CENTER_CENTER)
        .default_pos(ctx.content_rect().center())
        .resizable(true)
        .collapsible(false)
        .default_size([640.0, 480.0])
        .min_size([320.0, 240.0])
        .show(ctx, |ui| {
            preview_pane(app, ui);
        });
    if !open {
        close(&mut app.preview);
        return;
    }
    let ppp = ctx.pixels_per_point();
    sync_web(&mut app.preview, ppp);
    if app.preview.webview.is_some() {
        ctx.request_repaint();
    }
    if poll_webview_keys(&mut app.preview) {
        close(&mut app.preview);
    }
}

fn poll_webview_keys(preview: &mut PreviewState) -> bool {
    if preview.webview.is_none() {
        return false;
    }
    let space = async_key_down(0x20);
    let esc = async_key_down(0x1B);
    if !space {
        preview.space_armed = true;
    }
    let space_hit = preview.space_armed && space && !preview.prev_space_down;
    let esc_hit = esc && !preview.prev_esc_down;
    preview.prev_space_down = space;
    preview.prev_esc_down = esc;
    space_hit || esc_hit
}

fn async_key_down(vk: i32) -> bool {
    unsafe { windows::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState(vk) < 0 }
}

fn preview_pane(app: &mut ScApp, ui: &mut Ui) {
    let close_hint = format!("{} to close", app.settings.keymap.toggle_preview.label());
    let Some(path) = app.preview.path.clone() else {
        destroy_web(&mut app.preview);
        ui.weak("Select a file to preview");
        ui.add_space(8.0);
        ui.weak(&close_hint);
        return;
    };
    ui.label(RichText::new(path.file_name().unwrap_or_default().to_string_lossy()).strong());
    ui.weak(&close_hint);
    ui.add_space(4.0);
    if app.preview.loading {
        destroy_web(&mut app.preview);
        ui.spinner();
        return;
    }

    if app.preview.web_url.is_some() {
        if let Some(err) = app.preview.webview_error.clone() {
            destroy_web(&mut app.preview);
            ui.colored_label(app.theme.error, err);
            if let Some(text) = app.preview.web_fallback.clone() {
                ui.separator();
                draw_text(ui, &text);
            }
            return;
        }
        let (rect, _) = ui.allocate_exact_size(ui.available_size(), Sense::hover());
        app.preview.embed_rect = Some(rect);
        return;
    }
    destroy_web(&mut app.preview);

    if let Some(audio) = app.preview.audio.clone() {
        draw_audio(app, ui, &audio);
        return;
    }
    if let Some(tex) = &app.preview.texture {
        let avail = ui.available_size();
        let size = tex.size_vec2();
        let scale = (avail.x / size.x).min(avail.y.max(1.0) / size.y).min(1.0);
        ui.add(egui::Image::new(egui::load::SizedTexture::new(tex.id(), size * scale)));
        return;
    }
    if let Some(text) = app.preview.text.clone() {
        draw_text(ui, &text);
        return;
    }
    if let Some(hex) = app.preview.hex.clone() {
        draw_hex(ui, &hex);
        return;
    }
    if let Some(info) = &app.preview.info {
        ui.weak(info);
    }
}

fn draw_text(ui: &mut Ui, text: &str) {
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.add(
                egui::Label::new(RichText::new(text).monospace().size(12.0))
                    .wrap_mode(egui::TextWrapMode::Extend),
            );
        });
}

fn draw_hex(ui: &mut Ui, hex: &HexPreview) {
    let shown = hex.bytes.len() as u64;
    ui.weak(format!(
        "Hex · showing {} of {}",
        sc_core::entry::format_size(shown),
        sc_core::entry::format_size(hex.file_size)
    ));
    ui.add_space(4.0);
    let rows = hex.bytes.len().div_ceil(HEX_COLS).max(1);
    egui::ScrollArea::both()
        .auto_shrink([false, false])
        .show_rows(ui, 16.0, rows, |ui, range| {
            ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
            for i in range {
                let start = i * HEX_COLS;
                let end = (start + HEX_COLS).min(hex.bytes.len());
                ui.add(
                    egui::Label::new(
                        RichText::new(hex_row(start, &hex.bytes[start..end])).monospace().size(12.0),
                    )
                    .selectable(true),
                );
            }
        });
}

pub fn hex_row(offset: usize, bytes: &[u8]) -> String {
    let mut hex = String::with_capacity(16 * 3 + 2);
    let mut ascii = String::with_capacity(16);
    for i in 0..HEX_COLS {
        if i == 8 {
            hex.push(' ');
        }
        if let Some(&b) = bytes.get(i) {
            hex.push_str(&format!("{b:02X} "));
            ascii.push(if (0x20..0x7F).contains(&b) { b as char } else { '.' });
        } else {
            hex.push_str("   ");
            ascii.push(' ');
        }
    }
    format!("{offset:08X}  {hex} {ascii}")
}

fn draw_audio(app: &mut ScApp, ui: &mut Ui, audio: &AudioPreview) {
    ui.horizontal(|ui| {
        if let Some(tex) = &app.preview.texture {
            let size = tex.size_vec2();
            let side = 128.0_f32.min(size.x).min(size.y);
            ui.add(egui::Image::new(egui::load::SizedTexture::new(
                tex.id(),
                egui::vec2(side, side),
            )));
        }
        ui.vertical(|ui| {
            for (k, v) in &audio.lines {
                ui.horizontal(|ui| {
                    ui.weak(format!("{k}:"));
                    ui.label(v);
                });
            }
            if let Some(secs) = audio.duration_secs {
                ui.weak(format!("Duration: {}", format_duration(secs)));
            }
        });
    });
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        let playing = app.preview.audio_ctl.is_playing();
        let paused = app.preview.audio_ctl.is_paused();
        if playing {
            if ui.button("Pause").clicked() {
                app.preview.audio_ctl.pause();
            }
        } else if paused {
            if ui.button("Resume").clicked() {
                app.preview.audio_ctl.resume();
            }
        } else if ui.button("Play").clicked() {
            app.preview.audio_ctl.play(audio.path.clone());
        }
        if ui.button("Stop").clicked() {
            app.preview.audio_ctl.stop();
        }
    });
    if let Some(err) = app.preview.audio_ctl.last_error() {
        ui.colored_label(app.theme.error, err);
    }
}

fn format_duration(secs: f64) -> String {
    if !secs.is_finite() || secs < 0.0 {
        return "--:--".into();
    }
    let t = secs.round() as u64;
    let h = t / 3600;
    let m = (t % 3600) / 60;
    let s = t % 60;
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

pub fn clear_content(preview: &mut PreviewState) {
    preview.texture = None;
    preview.text = None;
    preview.info = None;
    preview.hex = None;
    preview.audio = None;
    preview.web_url = None;
    preview.web_fallback = None;
    preview.webview_error = None;
    preview.embed_rect = None;
}

#[cfg(test)]
mod tests {
    use super::hex_row;

    #[test]
    fn hex_row_formats_offset_hex_and_ascii() {
        let line = hex_row(0, b"PNG\n");
        assert!(line.starts_with("00000000"));
        assert!(line.contains("50 4E 47"));
        assert!(line.contains("PNG"));
    }
}
