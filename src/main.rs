#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod analysis;
mod app;
mod athlete;
mod geo;
mod io;
mod model;
mod platform;
mod ui;

use app::App;

/// The app icon, embedded so the running window shows it in the taskbar/Dock
/// (on macOS the bundle's `.icns` drives the Dock; this covers Windows/Linux and
/// running the bare binary).
#[cfg(not(target_arch = "wasm32"))]
const ICON_PNG: &[u8] = include_bytes!("../app_icon.png");

#[cfg(not(target_arch = "wasm32"))]
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

/// Web entry point: mount the app on the `<canvas id="legwork_canvas">` in
/// `index.html`. `main` returns `()` on wasm; the runner drives the frame loop.
#[cfg(target_arch = "wasm32")]
fn main() {
    // Surface Rust panics in the browser console instead of an opaque "unreachable".
    std::panic::set_hook(Box::new(|info| {
        web_sys::console::error_1(&format!("panic: {info}").into());
    }));

    wasm_bindgen_futures::spawn_local(async {
        let canvas = web_sys::window()
            .and_then(|w| w.document())
            .and_then(|d| d.get_element_by_id("legwork_canvas"))
            .and_then(|c| c.dyn_into::<web_sys::HtmlCanvasElement>().ok())
            .expect("index.html must contain <canvas id=\"legwork_canvas\">");

        let result = eframe::WebRunner::new()
            .start(
                canvas,
                eframe::WebOptions::default(),
                Box::new(|cc| Ok(Box::new(App::new(cc)))),
            )
            .await;
        if let Err(e) = result {
            web_sys::console::error_1(&format!("failed to start eframe: {e:?}").into());
        }
    });
}

#[cfg(target_arch = "wasm32")]
use wasm_bindgen::JsCast as _;
