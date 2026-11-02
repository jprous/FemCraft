// Hide the console window in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod camera;
mod grid;
mod viewport;

use eframe::egui;

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("FemCraft")
            .with_inner_size([1400.0, 900.0])
            .with_min_inner_size([800.0, 500.0]),
        renderer: eframe::Renderer::Wgpu,
        multisampling: viewport::MSAA_SAMPLES as u16,
        ..Default::default()
    };

    eframe::run_native(
        "FemCraft",
        options,
        Box::new(|cc| Ok(Box::new(app::FemCraftApp::new(cc)))),
    )
}
