# Legwork

Orienteering analysis app in Rust inspired by QuickRoute.

Load a photo or scan of your map, drop a GPX/TCX track on top, georeference it by
dragging a few route points onto matching map features, color the route by pace/speed,
and analyze your legs.

## Features

- **Map image**: open JPEG / PNG / GIF / TIFF / BMP / WebP; pan & zoom canvas.
- **Track import**: unified GPX and TCX parser (captures time, elevation, and heart rate
  from GPX `gpxtpx` extensions and TCX `HeartRateBpm`).
- **Georeferencing**: drag route points onto matching map features to lock them. Each
  locked point is honored *exactly* — 1 → translation, 2 → similarity, 3+ →
  **interpolating thin-plate spline** that warps the surrounding route locked points
  (handles angled phone photos), so previously-placed points never drift. Live
  fit-residual readout.
- **Coloring**: route segments colored **blue (slow) → red (quick)** by pace. In Controls
  mode the right pane has a **Coloring** palette that combines the pace scale (min/km) with
  the color gradient: drag the blue (slow) and red (quick) handles on the palette to set the
  cutoffs, or use "Auto" to fit the range to the run.
- **Run graphs**: in Controls view, pace, heart-rate and elevation graphs of the run appear at
  the bottom (vertical marks at each control), each toggled individually from the **Graphs**
  section in the right pane. Hovering a graph shows the value at that point and a shared
  cursor across all graphs, and highlights the matching spot on the route; hovering the route
  moves the graph cursor too.
- **Controls & legs**: click to place a control, right-click to remove one; the legs table
  shows per-leg time, route length, detour %, and pace.
- **Persistence**: save/open a single `.legit` container (zip of image + track + JSON).
  Projects saved as `.route` before the rename still open.
- **Export**: render the analyzed map (route + controls burned in) to PNG.

## Build & run

```sh
cargo run --release
```

On Linux you need the usual GUI dev packages: `libgtk-3-dev`, `libxcb-render0-dev`,
`libxcb-shape0-dev`, `libxcb-xfixes0-dev`, `libxkbcommon-dev`, `libssl-dev`.

### Try it

1. Run the app.
2. **Open Map…** and pick a photo/scan of an orienteering map.
3. **Open Track…** and pick `samples/example.gpx` (or your own GPX/TCX).
   The route appears overlaid via an initial bounding-box fit.
4. Switch to **Calibrate** mode. Press on a point of the route and drag it onto the
   matching feature on the map, then release to lock it. Add more points the same way:
   one point translates the route, two rotate/scale it, three or more warp it (TPS) so
   every locked point stays exactly on its feature.
5. Switch to **Controls** mode and click along the route to place controls; read the legs
   table on the right.
6. **Save Project…** to a `.legit` file, or **Export PNG…**.

Drag empty map space to pan and scroll to zoom in either mode.

## Architecture

```
src/
  main.rs            eframe entry point
  app.rs             App state + eframe::App impl, coordinate + transform plumbing
  model/             Waypoint/Track, project (serde) types
  io/                GPX/TCX parser, image loader, .legit container, PNG export
  geo/               local projection + similarity / TPS interpolating warp solving
  analysis/          per-leg metrics, pace/speed coloring
  ui/                map canvas (pan/zoom/drag) and side/top panels
```

Pure-logic layers (`model`, `geo`, `analysis`, `io::track_import`) are unit-tested:
`cargo test`.

## Packaging

### macOS (Apple Silicon)

```sh
scripts/macos/bundle-macos.sh
```

Builds a native `aarch64-apple-darwin` release, wraps it in a double-clickable
`dist/Legwork.app` (icon built from `app_icon.png` + `Info.plist`, ad-hoc signed), and
zips it as `dist/Legwork-<version>-aarch64-apple-darwin.zip`.

The app icon lives at `app_icon.png` (a 1024px master) in the project root; the bundle
script derives the `.icns` from it at build time.

The bundle is ad-hoc signed, not notarized, so on first launch Gatekeeper will block it.
Right-click the app → **Open** (or run `xattr -dr com.apple.quarantine dist/Legwork.app`)
to allow it. For distribution to other machines, sign with a Developer ID certificate and
notarize.

### Windows (x86_64)

```powershell
scripts\windows\bundle-windows.ps1
```

Builds an `x86_64-pc-windows-msvc` release binary (with `app_icon.ico` embedded
into the `.exe` by `build.rs`, so Explorer and the taskbar show the icon) and
wraps it in an MSI installer via [`cargo-wix`](https://github.com/volks73/cargo-wix)
and the WiX 3 Toolset.

Output: `target\wix\legwork-<version>-x86_64.msi`. The installer places Legwork in
`Program Files`, adds a **Legwork** Start Menu shortcut, and registers it (with the
app icon) in *Apps & features* for clean uninstall/upgrade.

On first run the script installs `cargo-wix` and downloads the standalone WiX 3
binaries into `%LOCALAPPDATA%\WiX314` automatically — no admin rights needed. Pass
`-NoBuild` to package an already-built `target\release\legwork.exe`.

The MSI is unsigned, so on first launch Windows SmartScreen may show a
"Windows protected your PC" prompt — click **More info → Run anyway**. For
distribution, sign the `.exe` and `.msi` with an Authenticode code-signing
certificate.

The installer definition lives at `wix\main.wxs`; edit it to change the install
layout, shortcuts, or add a EULA.

### Other platforms

For a single cross-platform installer pipeline (`.dmg` / `.msi` / tarball) add
[`cargo-dist`](https://github.com/axodotdev/cargo-dist): `cargo dist init`.
