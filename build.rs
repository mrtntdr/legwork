// Embed the Windows icon + version metadata into legwork.exe so File Explorer,
// the taskbar and Add/Remove Programs show the app icon and details. No-op on
// non-Windows targets (macOS gets its icon from the .app bundle's .icns).
fn main() {
    #[cfg(windows)]
    {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("app_icon.ico");
        res.set("ProductName", "Legwork");
        res.set("FileDescription", "Legwork — orienteering analysis");
        res.compile().expect("failed to embed Windows resources");
    }
}
