# Version history

Feature history of Legwork, newest first. See `Cargo.toml` for the current version.

## 0.1.1 — Analysis board (drawn route options)

Draw and measure alternative route choices directly on the map.

- **Route drawing** on the Analysis tab, behind an explicit ✏ Draw toggle (leg strip,
  side panel, or the `D` key) so the map stays read-only for the course otherwise.
  - **Click** to drop vertices one by one; **drag** to sketch freehand (samples are
    simplified with Douglas–Peucker). The two modes feed the same route.
  - Double-click or **Enter** to finish, **Esc** to cancel, **Ctrl/Cmd+Z** to step back
    one action. Drag a vertex to adjust; right-click a vertex to delete it.
- **Real-world distance** for each drawn route, in meters. Measured through the map's
  georeferencing when present, otherwise by inverting a calibrated athlete's transform
  (needs ≥2 calibration pins); shows "— m" until the map can be measured.
- **Leg-attached variants**: with a leg selected, drawn routes attach to it and their
  endpoints snap to the leg's controls. The side panel lists each variant's length and
  its delta to the shortest, alongside the athletes' actual distances on that leg.
- **Free-form routes**: draw anywhere for a general measuring tool, independent of the
  course.
- **Rogaine / score-O**: controls can carry point values (Setup · Course · Scores). A
  drawn route detects which controls it passes and shows distance, points collected, and
  points per km — so you can compare which route choice yields the most points.
- Drawn routes and control scores are **saved in the `.legit` project**, added
  backward-compatibly (old projects still open; route-less projects serialize unchanged).
- Add map rotation functionality

## 0.1.x — Foundation

- Multi-athlete leg-by-leg analysis: load a map photo/scan, drop GPX/TCX tracks,
  georeference them, define the course, and compare athletes on the map, in a splits
  table, and as an animated replay.
- World-file / GeoTIFF georeferencing and IOF XML 3.0 course import.
- Per-athlete calibration (translate / similarity / TPS warp), pace coloring, and the
  Setup/Analysis two-tab workflow.
- `.legit` project container with transparent V1 (single-track) migration.
