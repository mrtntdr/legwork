# Legwork

Orienteering analysis app in Rust inspired by QuickRoute and Livelox.

Load a photo or scan of your map, drop one or more GPX/TCX tracks on top, georeference
them by dragging a few route points onto matching map features, define the course, and
compare athletes leg by leg — on the map, in a splits table, and as an animated replay.

The app is organized around two activities:

- **Setup** — load the map and tracks, calibrate each athlete, place the course.
- **Analysis** — the map is the read-only centerpiece: step through legs, replay the
  race, and open the splits/graphs drawers. A saved project opens straight here.

## Features

### Setup

- **Map image**: open JPEG / PNG / GIF / TIFF / BMP / WebP; pan & zoom canvas.
- **Track import**: unified GPX and TCX parser (captures time, elevation, and heart rate
  from GPX `gpxtpx` extensions and TCX `HeartRateBpm`).
- **Multiple athletes**: add any number of tracks against one shared map. Each athlete
  has a name, a route color, a visibility toggle, and their **own calibration**, so GPS
  offsets between different watches are corrected per person. One *active* athlete
  drives calibration, graphs, and pace coloring.
- **Georeferencing** (Calibrate mode): drag route points onto matching map features to
  lock them. Each locked point is honored *exactly* — 1 → translation, 2 → similarity,
  3+ → **interpolating thin-plate spline** that warps the route (handles angled phone
  photos), so previously-placed points never drift. Live fit-residual readout. A newly
  added athlete borrows the best-calibrated athlete's transform, so it lands roughly
  aligned before its first pin.
- **Shared course** (Course mode): click the map to place controls, drag to move them,
  right-click to remove. Every athlete's track is automatically matched to its nearest
  pass by each control (scale-aware radius, monotone along the track), so legs are
  comparable across athletes; a control someone never visits gets a warning ring.

### Analysis

- **Leg view on the map**: a strip above the map steps through the course leg by leg
  (arrows, clickable labels, or ←/→). Selecting a leg zooms to it, draws only each
  athlete's route choice for that leg, and dims the other controls. Clicking a control
  on the map jumps to its leg; `Esc` returns to the whole course.
- **Replay animation**: athletes as moving dots with bright tails over faint routes —
  play/pause (`Space`), draggable timeline, speed (1–120×), tail length, and a **Solo**
  toggle for a single athlete. Start modes: **Mass start** (everyone's clock zeroed at
  their own start), **Real time** (actual wall-clock offsets), and with a leg selected
  everyone **restarts together at that control** (Livelox-style).
- **Splits drawer**: a leg-by-leg comparison table — per-athlete time with delta to the
  best split (fastest in green), pace and length, optional cumulative times with gaps,
  and a total row. A missed control blanks only its adjacent legs; cumulative gaps
  recover at the next matched control. Clicking a leg's label zooms the map to it.
- **Leaderboard / leg summary**: with the whole course shown, the side panel ranks
  visible athletes by total time with gaps to the leader; with a leg selected it shows
  that leg's times, deltas, pace, and length.
- **Coloring**: the active route can be colored **blue (slow) → red (quick)** by pace;
  the palette combines the pace scale (min/km) with the gradient — drag the handles to
  set cutoffs, or use "Auto" to fit the range to the run.
- **Graphs drawer**: pace, heart-rate and elevation graphs of the active athlete's run
  (vertical marks at each matched control). Hovering a graph shows a shared cursor
  across all graphs and highlights the matching spot on the route, and vice versa.

### Files

- **Persistence**: save/open a single `.legit` container (zip of image + tracks + JSON)
  holding all athletes, calibrations, and the course. Old single-track projects
  (including pre-rename `.route` files) still open and are migrated transparently.
- **Export**: render the analyzed map (all visible routes + controls burned in) to PNG.

## Build & run

```sh
cargo run --release
```

On Linux you need the usual GUI dev packages: `libgtk-3-dev`, `libxcb-render0-dev`,
`libxcb-shape0-dev`, `libxcb-xfixes0-dev`, `libxkbcommon-dev`, `libssl-dev`.

### Try it

1. Run the app (it opens on the **Setup** tab).
2. **File → Open Map…** and pick a photo/scan of an orienteering map.
3. **File → Add Track…** and pick `samples/example.gpx` (or your own GPX/TCX).
   The route appears overlaid via an initial bounding-box fit. Add more tracks the
   same way — each becomes an athlete with its own color.
4. In **Calibrate** mode, press on a point of the active route and drag it onto the
   matching feature on the map, then release to lock it. Add more points the same way:
   one point translates the route, two rotate/scale it, three or more warp it (TPS) so
   every locked point stays exactly on its feature. Other athletes inherit the
   alignment; pick each one in the Athletes list to fine-tune their own pins.
5. In **Course** mode, click the map to place controls along the route. Every
   athlete is matched to the course automatically.
6. Switch to the **Analysis** tab: step through legs with the strip above the map
   (or ←/→), open the **Splits** and **Graphs** drawers from the bottom bar, and
   tick **Replay** to animate the race (`Space` to play/pause).
7. **File → Save Project…** to a `.legit` file, or **File → Export PNG…**.

Drag empty map space to pan and scroll to zoom on either tab.

## Architecture

```
src/
  main.rs            eframe entry point
  app.rs             App state + eframe::App impl, coordinate + transform plumbing,
                     leg selection and replay orchestration
  athlete.rs         per-athlete runtime state (track, calibration, transform,
                     control matches, replay timeline)
  model/             Waypoint/Track, project (serde) types incl. V1→V2 migration
  io/                GPX/TCX parser, image loader, .legit container, PNG export
  geo/               local projection + similarity / TPS interpolating warp solving
  analysis/          per-leg metrics, control↔track matching, cross-athlete
                     comparison, replay timing, pace/speed coloring
  ui/                map canvas (pan/zoom/drag, leg view, replay rendering),
                     top/side panels, splits & graphs drawers, transport bar
```

Pure-logic layers (`model`, `geo`, `analysis`, `io`) are unit-tested: `cargo test`.

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
