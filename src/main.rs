#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod analysis;
mod app;
mod geo;
mod io;
mod model;
mod ui;

use app::App;

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1280.0, 860.0])
            .with_title("Legwork — orienteering analysis"),
        ..Default::default()
    };
    eframe::run_native(
        "legwork",
        options,
        Box::new(|cc| Ok(Box::new(App::new(cc)))),
    )
}
