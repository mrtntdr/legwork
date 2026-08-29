//! Browser file backend: `rfd::AsyncFileDialog` for opens, a `Blob` + synthetic
//! `<a download>` click for saves (browsers have no save dialog / filesystem).

use super::{FileRequest, FileSender, PickedFile, SaveKind};
use wasm_bindgen::JsCast;

/// Open a browser file picker for `req` and send the chosen file on `tx` once the
/// user has picked and the bytes have been read. Returns immediately.
pub fn pick_file(req: FileRequest, tx: FileSender, ctx: egui::Context) {
    wasm_bindgen_futures::spawn_local(async move {
        let mut dialog = rfd::AsyncFileDialog::new();
        dialog = match req {
            FileRequest::OpenMap => dialog.add_filter("Images", super::IMAGE_EXTS),
            FileRequest::AddTrack => dialog.add_filter("GPX/TCX", super::TRACK_EXTS),
            FileRequest::ImportCourse => dialog.add_filter("IOF XML course", &["xml"]),
            FileRequest::OpenProject => dialog.add_filter("Legwork project", &["legit", "route"]),
        };
        if let Some(handle) = dialog.pick_file().await {
            let name = handle.file_name();
            let bytes = handle.read().await;
            let _ = tx.send(PickedFile {
                request: req,
                name,
                bytes,
                path: None,
            });
            ctx.request_repaint();
        }
    });
}

/// Trigger a browser download of `bytes`. Always `Ok(true)` — the browser owns the
/// rest of the flow once the download starts.
pub fn save_file(kind: SaveKind, suggested_name: &str, bytes: Vec<u8>) -> Result<bool, String> {
    let mime = match kind {
        SaveKind::Project => "application/zip",
        SaveKind::Png => "image/png",
    };
    download(suggested_name, mime, &bytes).map(|_| true)
}

fn download(name: &str, mime: &str, bytes: &[u8]) -> Result<(), String> {
    let window = web_sys::window().ok_or("no window")?;
    let document = window.document().ok_or("no document")?;

    // A Blob wrapping the bytes, with its content type set for the download.
    let array = js_sys::Uint8Array::from(bytes);
    let parts = js_sys::Array::new();
    parts.push(&array.buffer());
    let opts = web_sys::BlobPropertyBag::new();
    opts.set_type(mime);
    let blob = web_sys::Blob::new_with_u8_array_sequence_and_options(&parts, &opts)
        .map_err(|_| "failed to build blob")?;
    let url = web_sys::Url::create_object_url_with_blob(&blob).map_err(|_| "failed to make url")?;

    // A detached <a download> whose click the browser turns into a file save.
    let anchor: web_sys::HtmlAnchorElement = document
        .create_element("a")
        .map_err(|_| "create anchor")?
        .dyn_into()
        .map_err(|_| "anchor cast")?;
    anchor.set_href(&url);
    anchor.set_download(name);
    anchor.set_attribute("style", "display:none").ok();
    let body = document.body().ok_or("no body")?;
    body.append_child(&anchor).map_err(|_| "append anchor")?;
    anchor.click();
    body.remove_child(&anchor).ok();
    web_sys::Url::revoke_object_url(&url).ok();
    Ok(())
}
