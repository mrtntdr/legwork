//! Desktop file backend: native OS dialogs via `rfd` + `std::fs`.
//!
//! The picker runs synchronously on the calling (UI) thread — rfd's blocking API
//! must be used from the main thread on macOS — and delivers the result through
//! the same channel the web backend uses, so `App` has one code path.

use super::{FileRequest, FileSender, PickedFile, SaveKind, IMAGE_EXTS, TRACK_EXTS};

/// Open a native file picker for `req` and send the chosen file on `tx`. Blocks
/// while the dialog is open (as it always has); a cancel simply sends nothing.
pub fn pick_file(req: FileRequest, tx: FileSender, ctx: egui::Context) {
    let mut dialog = rfd::FileDialog::new();
    dialog = match req {
        FileRequest::OpenMap => dialog.add_filter("Images", IMAGE_EXTS),
        FileRequest::AddTrack => dialog.add_filter("GPX/TCX", TRACK_EXTS),
        FileRequest::ImportCourse => dialog.add_filter("IOF XML course", &["xml"]),
        FileRequest::OpenProject => dialog.add_filter("Legwork project", &["legit", "route"]),
    };
    if let Some(path) = dialog.pick_file()
        && let Ok(bytes) = std::fs::read(&path)
    {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "file".into());
        let _ = tx.send(PickedFile {
            request: req,
            name,
            bytes,
            path: Some(path),
        });
        ctx.request_repaint();
    }
}

/// Pop a save dialog and write `bytes`. `Ok(true)` = saved, `Ok(false)` = the user
/// cancelled, `Err` = a write failure.
pub fn save_file(kind: SaveKind, suggested_name: &str, bytes: Vec<u8>) -> Result<bool, String> {
    let mut dialog = rfd::FileDialog::new().set_file_name(suggested_name);
    dialog = match kind {
        SaveKind::Project => dialog.add_filter("Legwork project", &["legit"]),
        SaveKind::Png => dialog.add_filter("PNG", &["png"]),
    };
    match dialog.save_file() {
        Some(path) => std::fs::write(&path, bytes)
            .map(|_| true)
            .map_err(|e| e.to_string()),
        None => Ok(false),
    }
}
