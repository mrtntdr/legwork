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

On Linux you need the usual GUI dev packages (see `.github/workflows/ci.yml` for the
exact list: GTK3, xcb, xkbcommon).

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

CI builds and tests on all three OSes (`.github/workflows/ci.yml`). For distributable
installers (`.dmg` / `.msi` / tarball) add [`cargo-dist`](https://github.com/axodotdev/cargo-dist):
`cargo dist init`.
