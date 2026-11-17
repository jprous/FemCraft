// Hide the console window in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use eframe::egui;

fn main() -> eframe::Result<()> {
    // `femcraft --mcp`: MCP server on stdin / stdout, no window.
    if std::env::args().any(|a| a == "--mcp") {
        let mut ws = femcraft_mcp::headless::Headless::new(
            femcraft_ui::FemCraftApp::blank_model_path(),
            femcraft_ui::FemCraftApp::prototype_files(),
        );
        if let Err(e) = femcraft_mcp::stdio::run(&mut ws) {
            eprintln!("femcraft --mcp: {e}");
        }
        return Ok(());
    }
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("FemCraft")
            .with_icon(
                eframe::icon_data::from_png_bytes(include_bytes!(
                    "../../../assets/icon/femcraft-256.png"
                ))
                .expect("valid icon PNG"),
            )
            .with_inner_size([1400.0, 900.0])
            .with_min_inner_size([800.0, 500.0]),
        renderer: eframe::Renderer::Wgpu,
        multisampling: femcraft_render::viewport::MSAA_SAMPLES as u16,
        ..Default::default()
    };

    eframe::run_native(
        "FemCraft",
        options,
        Box::new(|cc| Ok(Box::new(femcraft_ui::FemCraftApp::new(cc)))),
    )
}
