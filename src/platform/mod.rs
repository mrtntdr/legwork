//! File open/save that works the same way on desktop and in the browser.
//!
//! egui is immediate-mode and the UI loop must never block on a picker, so opens
//! are asynchronous everywhere: [`pick_file`] starts a picker and the chosen file
//! arrives later on an `mpsc` channel as a [`PickedFile`], which the app drains at
//! the top of each frame. Saves are synchronous — native pops a save dialog and
//! writes, web triggers a browser download.
//!
//! The two backends live in `native` and `web`; only one compiles per target.

use std::path::PathBuf;
use std::sync::mpsc::Sender;

#[cfg(not(target_arch = "wasm32"))]
mod native;
#[cfg(not(target_arch = "wasm32"))]
pub use native::{pick_file, save_file};

#[cfg(target_arch = "wasm32")]
mod web;
#[cfg(target_arch = "wasm32")]
pub use web::{pick_file, save_file};

/// Which "Open…" action a picked file is answering, so the app knows how to
/// interpret the bytes without threading extra state through the channel.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum FileRequest {
    OpenMap,
    AddTrack,
    ImportCourse,
    OpenProject,
}

/// A file the user picked. `path` is only ever `Some` on native — the web backend
/// hands back bytes with no filesystem path (used for world-file georef sidecars).
pub struct PickedFile {
    pub request: FileRequest,
    pub name: String,
    pub bytes: Vec<u8>,
    pub path: Option<PathBuf>,
}

/// What is being saved, which selects the file dialog filter (native) / download
/// MIME type (web).
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SaveKind {
    Project,
    Png,
}

pub(crate) const IMAGE_EXTS: &[&str] = &["jpg", "jpeg", "png", "gif", "tiff", "tif", "bmp", "webp"];
pub(crate) const TRACK_EXTS: &[&str] = &["gpx", "tcx", "xml"];

/// The channel type the app owns to receive picked files.
pub type FileSender = Sender<PickedFile>;
