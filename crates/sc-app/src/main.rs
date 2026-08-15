#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod config;
mod dialogs;
mod icons;
mod jobs;
mod keymap;
mod preview;
mod settings_ui;
mod sidebar;
mod tags;
mod theme;
mod ui;
mod vfs;

use app::ScApp;

impl eframe::App for ScApp {
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        self.preview.parent_hwnd = crate::preview::capture_parent(frame);
        let ctx = ui.ctx().clone();
        self.pump_messages(&ctx);
        ui::draw(self, ui);
        // Periodic session autosave (crash safety).
        let interval = self.settings.autosave_secs.max(5);
        if self.last_session_save.elapsed().as_secs() > interval {
            self.save_session();
        }
    }

    fn on_exit(&mut self) {
        crate::preview::shutdown(&mut self.preview);
        self.save_session();
        self.engine.index.shutdown();
    }
}

fn set_app_user_model_id() {
    unsafe {
        let _ = windows::Win32::UI::Shell::SetCurrentProcessExplicitAppUserModelID(
            windows::core::w!("SimpleCommander.App"),
        );
    }
}

fn app_icon() -> egui::IconData {
    eframe::icon_data::from_png_bytes(include_bytes!("../assets/icon.png"))
        .expect("app icon")
}

fn main() -> eframe::Result {
    use eframe::egui_wgpu::{wgpu, WgpuSetup};
    set_app_user_model_id();
    let launched = std::time::Instant::now();
    // Restrict to DX12 with a GL fallback: enumerating Vulkan crashes inside
    // some Windows Vulkan drivers before we get any chance to recover.
    // WGPU_BACKEND still overrides for debugging.
    let mut wgpu_options = eframe::egui_wgpu::WgpuConfiguration::default();
    if let WgpuSetup::CreateNew(setup) = &mut wgpu_options.wgpu_setup {
        setup.instance_descriptor.backends = wgpu::Backends::from_env()
            .unwrap_or(wgpu::Backends::DX12 | wgpu::Backends::GL);
    }
    // Frame latency 1 causes visible flicker on some DX12 drivers whenever the
    // cursor triggers repaints; latency 2 with vsync is stable.
    wgpu_options.surface = eframe::egui_wgpu::SurfaceConfig::HIGH_THROUGHPUT;
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("SimpleCommander")
            .with_app_id("SimpleCommander.App")
            .with_icon(app_icon())
            .with_inner_size([1280.0, 800.0])
            .with_min_inner_size([720.0, 480.0]),
        wgpu_options,
        ..Default::default()
    };
    eframe::run_native(
        "SimpleCommander",
        options,
        Box::new(move |cc| {
            let mut app = ScApp::new(cc);
            app.start_time = launched;
            Ok(Box::new(app))
        }),
    )
}
