#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod analysis;
mod app;
mod geo;
mod io;
mod model;
mod ui;

use app::App;

/// The app icon, embedded so the running window shows it in the taskbar/Dock
/// (on macOS the bundle's `.icns` drives the Dock; this covers Windows/Linux and
/// running the bare binary).
const ICON_PNG: &[u8] = include_bytes!("../app_icon.png");

fn main() -> eframe::Result<()> {
    let mut viewport = egui::ViewportBuilder::default()
        .with_inner_size([1280.0, 860.0])
        .with_title("Legwork — orienteering analysis");
    match eframe::icon_data::from_png_bytes(ICON_PNG) {
        Ok(icon) => viewport = viewport.with_icon(icon),
        Err(e) => eprintln!("failed to load app icon: {e}"),
    }

    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };
    eframe::run_native(
        "legwork",
        options,
        Box::new(|cc| Ok(Box::new(App::new(cc)))),
    )
}
