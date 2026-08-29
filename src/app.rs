use crate::analysis::{
    ClockMode, MetricRange, RouteStats, Window, auto_range, build_timeline, collected_controls,
    local_scale_px_per_m, match_controls, playback, route_midpoint_px, segment_metric, total_span,
};
use crate::athlete::{ATHLETE_COLORS, Athlete};
use crate::geo::{Correspondence, LocalProjection, MapTransform, invert_transform};
use crate::io;
use crate::io::{Crs, MapGeoref, ProjectBundle};
use crate::model::{AthleteFile, CoursePoint, DrawnRoute, ProjectFileV2, ViewState, Waypoint, haversine};
use crate::platform::{self, FileRequest, FileSender, PickedFile, SaveKind};
use chrono::{DateTime, Utc};
use egui::{Color32, Pos2, TextureHandle, pos2};
use std::path::PathBuf;
use std::sync::mpsc::Receiver;

/// A decoded map image plus its GPU texture and original encoded bytes.
pub struct MapImage {
    pub texture: TextureHandle,
    pub size: [usize; 2],
    pub bytes: Vec<u8>,
}

/// The two activities the app is organized around: **Setup** (load the map and
/// tracks, calibrate, build the course) and **Analysis** (legs, replay, splits —
/// the map is read-only there so analysis clicks can't edit the project).
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ViewTab {
    Setup,
    Analysis,
}

/// What a Setup-tab canvas click/drag does. (Dragging empty space always pans.)
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum EditMode {
    Calibrate,
    Control,
}

/// Screen width class, recomputed each frame. `Narrow` (a phone, or a very small
/// window) switches to the map-first mobile layout: full-screen map with a single
/// bottom sheet and a bottom toolbar instead of the desktop side panel + drawers.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ScreenClass {
    Wide,
    Narrow,
}

/// Which bottom sheet (if any) is open in the mobile layout. Only one is ever
/// shown at a time so the map stays primary.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum MobileSheet {
    None,
    /// The side-panel content (athletes, calibration/course or analysis controls).
    Panel,
    Splits,
    Graphs,
    Transport,
}

/// What the active pointer drag is manipulating.
#[derive(Clone, Copy)]
pub enum DragTarget {
    View,
    Calibration(usize),
    Control(usize),
    /// Moving one vertex of a finished drawn route.
    RouteVertex { route: usize, vertex: usize },
    /// A freehand pen stroke feeding the current route draft.
    RouteSketch,
}

/// An in-progress drawn route. `points` are the committed image-pixel vertices;
/// `stroke` holds the raw samples of the current freehand drag before it's
/// simplified and appended; `checkpoints` records the vertex count before each
/// click/stroke so one undo removes exactly one action.
#[derive(Default)]
pub struct RouteDraft {
    pub points: Vec<[f64; 2]>,
    pub stroke: Vec<[f64; 2]>,
    pub checkpoints: Vec<usize>,
}

/// A pending view fit, applied on the next frame once the canvas rect is known.
#[derive(Clone, Copy)]
pub enum FitRequest {
    /// Fit the whole map image into the canvas.
    Map,
    /// Fit an image-pixel-space rectangle (min, max) into the canvas.
    Rect { min: (f64, f64), max: (f64, f64) },
}

/// Which clock the replay runs on (the user-facing choice; leg-restart is
/// implied when a leg is selected).
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum StartMode {
    MassStart,
    RealTime,
}

/// Replay animation state. Transient — never persisted.
pub struct Playback {
    pub enabled: bool,
    pub playing: bool,
    /// Global replay clock, in seconds.
    pub clock: f64,
    /// Playback speed multiplier (real seconds → replay seconds).
    pub speed: f64,
    pub mode: StartMode,
    /// Length of the bright trail behind each dot, in seconds
    /// (`f64::INFINITY` = the whole route so far).
    pub tail_secs: f64,
    /// Animate only the active athlete.
    pub solo: bool,
}

impl Default for Playback {
    fn default() -> Self {
        Self {
            enabled: false,
            playing: false,
            clock: 0.0,
            speed: 15.0,
            mode: StartMode::MassStart,
            tail_secs: 60.0,
            solo: false,
        }
    }
}

/// Matching radius around a control, in meters on the ground.
const MATCH_RADIUS_M: f64 = 60.0;

/// A bare lat/lon waypoint, for measuring great-circle distances.
fn ll(lat: f64, lon: f64) -> Waypoint {
    Waypoint {
        lat,
        lon,
        ..Waypoint::default()
    }
}

pub struct App {
    // Loaded data
    pub(crate) map: Option<MapImage>,
    pub(crate) athletes: Vec<Athlete>,
    /// Index of the active athlete: the one being calibrated, graphed and
    /// pace-colored. Route editing and the graphs always follow this athlete.
    pub(crate) active: usize,
    /// Shared local projection, anchored at the first loaded track's centroid so
    /// every athlete's transform maps the same meter frame to image pixels (which
    /// lets an uncalibrated athlete borrow a calibrated athlete's transform).
    pub(crate) proj: Option<LocalProjection>,
    /// Map georeferencing from a world file or GeoTIFF, when the map has one.
    pub(crate) georef: Option<MapGeoref>,
    /// The georef expressed as a shared meters→pixels transform (an affine fit of
    /// the projection chain over the loaded tracks' extent). Used as the base for
    /// every athlete without their own calibration.
    georef_mt: Option<MapTransform>,
    /// The shared course: controls placed on the map, in course order.
    pub(crate) controls: Vec<CoursePoint>,
    /// User-drawn route options (the analysis board). Persisted.
    pub(crate) drawn_routes: Vec<DrawnRoute>,
    /// Derived per-route stats (length, collected controls, points), parallel to
    /// `drawn_routes`. Rebuilt by `recompute_drawn_stats`; never persisted.
    pub(crate) drawn_stats: Vec<RouteStats>,
    /// Cached pixels→meters transform (inverse of a calibrated athlete's map
    /// transform), used to measure drawn routes when the map isn't georeferenced.
    px_to_m: Option<MapTransform>,
    pub(crate) metric_range: MetricRange,
    /// When true, the coloring range is auto-fit to the data; when false the user
    /// has set the fast/slow cutoffs manually.
    pub(crate) color_auto: bool,
    /// While a coloring-palette knob is being dragged, the frozen bar range
    /// `(lo, hi)` in min/km. `None` at rest, so the bar reframes to the cutoffs
    /// once the drag is released.
    pub(crate) palette_view: Option<(f64, f64)>,
    /// Optional ceiling (min/km) for the pace graph's y-axis, set with the trim strip
    /// to clip slow spikes (e.g. long map-reading pauses) and zoom into the real data.
    /// `None` fits the axis to all data.
    pub(crate) pace_cap_minkm: Option<f64>,

    // View + interaction
    pub(crate) tab: ViewTab,
    pub(crate) view: ViewState,
    pub(crate) mode: EditMode,
    pub(crate) drag: Option<DragTarget>,
    /// True while a two-finger (multi-touch) gesture is driving the map, so
    /// single-pointer editing is suppressed and any in-flight drag is cancelled.
    pub(crate) gesturing: bool,
    /// Screen offset from the fingertip to a grabbed marker at drag start, so a
    /// touch drag doesn't snap the marker to the finger center (see `drag_pos`).
    pub(crate) grab_offset: egui::Vec2,
    /// Long-press (right-click substitute) detector for the map canvas.
    pub(crate) long_press: crate::ui::touch::LongPress,
    /// Set when a long-press has consumed the current press, so the tap egui emits
    /// on release doesn't also place/select something.
    pub(crate) swallow_tap: bool,
    /// Recomputed each frame from the window width; drives the mobile layout.
    pub(crate) screen: ScreenClass,
    /// Latched `true` the first time a touch is seen, so touch laptops / phones get
    /// finger-sized hit targets and controls even in a wide window.
    pub(crate) touch: bool,
    /// The open mobile bottom sheet (narrow layout only).
    pub(crate) sheet: MobileSheet,
    pub(crate) show_pace: bool,
    pub(crate) show_hr: bool,
    pub(crate) show_ele: bool,
    /// Draw the active athlete's route with pace coloring instead of its solid color.
    pub(crate) active_pace_colors: bool,
    /// Show cumulative times in the leg comparison table.
    pub(crate) show_cumulative: bool,
    /// The Splits drawer (leg-by-leg comparison table) in the Analysis tab.
    pub(crate) show_splits: bool,
    /// The Graphs drawer (pace/HR/elevation) in the Analysis tab.
    pub(crate) show_graphs: bool,
    /// A pending view fit, applied next frame; `None` at rest.
    pub(crate) fit: Option<FitRequest>,
    /// A pending absolute view rotation (radians), applied next frame about the
    /// canvas center once its rect is known; `None` at rest.
    pub(crate) pending_rotate: Option<f32>,
    /// Selected leg for the on-map leg view (0-based leg index), or `None` for
    /// the whole-course view. Leg `li` runs from boundary `li` to `li + 1`.
    pub(crate) selected_leg: Option<usize>,
    /// Analysis-tab draw mode: click/drag on the map sketches route options.
    pub(crate) draw_mode: bool,
    /// The route being drawn right now, if any.
    pub(crate) draft: Option<RouteDraft>,
    /// The currently highlighted drawn route (index into `drawn_routes`).
    pub(crate) selected_route: Option<usize>,
    /// Replay animation state.
    pub(crate) playback: Playback,

    /// Cross-highlight between the graphs and the route: the along-track position
    /// (km) currently pointed at, its resolved waypoint index, and this frame's
    /// freshly-detected hover (committed to `hover_km` at end of frame).
    pub(crate) hover_km: Option<f64>,
    pub(crate) hover_index: Option<usize>,
    pub(crate) pending_hover: Option<f64>,

    // Files
    pub(crate) image_name: String,
    pub(crate) project_path: Option<PathBuf>,
    /// Async file-picker results land here (native pickers block briefly, web
    /// pickers resolve a future); drained at the top of each frame.
    file_tx: FileSender,
    file_rx: Receiver<PickedFile>,

    // UI
    pub(crate) status: String,
    /// Last status text shown as a mobile toast, and the `ctx` time it appeared,
    /// so the toast can fade out a few seconds after the status changes.
    pub(crate) toast_text: String,
    pub(crate) toast_time: f64,
}

impl App {
    pub fn new(_cc: &eframe::CreationContext<'_>) -> Self {
        let (file_tx, file_rx) = std::sync::mpsc::channel();
        Self {
            map: None,
            athletes: Vec::new(),
            active: 0,
            proj: None,
            georef: None,
            georef_mt: None,
            controls: Vec::new(),
            drawn_routes: Vec::new(),
            drawn_stats: Vec::new(),
            px_to_m: None,
            metric_range: MetricRange { min: 0.0, max: 1.0 },
            color_auto: true,
            palette_view: None,
            pace_cap_minkm: None,
            tab: ViewTab::Setup,
            view: ViewState::default(),
            mode: EditMode::Calibrate,
            drag: None,
            gesturing: false,
            grab_offset: egui::Vec2::ZERO,
            long_press: crate::ui::touch::LongPress::default(),
            swallow_tap: false,
            screen: ScreenClass::Wide,
            touch: false,
            sheet: MobileSheet::None,
            show_pace: true,
            show_hr: true,
            show_ele: true,
            active_pace_colors: true,
            show_cumulative: true,
            show_splits: false,
            show_graphs: false,
            fit: None,
            pending_rotate: None,
            selected_leg: None,
            draw_mode: false,
            draft: None,
            selected_route: None,
            playback: Playback::default(),
            hover_km: None,
            hover_index: None,
            pending_hover: None,
            image_name: String::new(),
            project_path: None,
            file_tx,
            file_rx,
            status: "Open a map image and add a GPS track to begin.".into(),
            toast_text: String::new(),
            toast_time: 0.0,
        }
    }

    // --- File requests (open/save through the platform layer) ----------------

    /// Start an async file picker for `req`; the result is delivered to
    /// `deliver_file` at the top of a later frame.
    pub(crate) fn request_file(&self, ctx: &egui::Context, req: FileRequest) {
        platform::pick_file(req, self.file_tx.clone(), ctx.clone());
    }

    /// Apply a picked file according to what was requested.
    fn deliver_file(&mut self, ctx: &egui::Context, f: PickedFile) {
        match f.request {
            FileRequest::OpenMap => {
                // A world-file sidecar or embedded GeoTIFF tags let tracks and IOF
                // courses land on the map with no calibration. Sidecars need a real
                // path (native only); the web path relies on embedded GeoTIFF tags.
                let georef = match &f.path {
                    Some(p) => crate::io::detect_georef(p, &f.bytes),
                    None => crate::io::parse_geotiff(&f.bytes),
                };
                self.load_image_from_bytes(ctx, f.bytes, f.name, georef);
            }
            FileRequest::AddTrack => self.add_athlete(f.bytes, f.name),
            FileRequest::ImportCourse => self.import_course(&f.bytes),
            FileRequest::OpenProject => self.open_project_bytes(ctx, f.bytes, f.path),
        }
    }

    // --- Athletes --------------------------------------------------------------

    pub(crate) fn active(&self) -> Option<&Athlete> {
        self.athletes.get(self.active)
    }

    pub(crate) fn active_mut(&mut self) -> Option<&mut Athlete> {
        self.athletes.get_mut(self.active)
    }

    pub(crate) fn set_active(&mut self, i: usize) {
        if i < self.athletes.len() && i != self.active {
            self.active = i;
            self.recompute_metric_active();
        }
    }

    pub(crate) fn remove_athlete(&mut self, i: usize) {
        if i >= self.athletes.len() {
            return;
        }
        self.athletes.remove(i);
        if self.active > i {
            self.active -= 1;
        }
        if self.active >= self.athletes.len() {
            self.active = self.athletes.len().saturating_sub(1);
        }
        if self.athletes.is_empty() {
            // The next track re-anchors the shared projection.
            self.proj = None;
        }
        self.refresh_georef_transform();
        self.recompute_metric_active();
    }

    // --- Loading -------------------------------------------------------------

    pub(crate) fn load_image_from_bytes(
        &mut self,
        ctx: &egui::Context,
        bytes: Vec<u8>,
        name: String,
        georef: Option<MapGeoref>,
    ) {
        match io::load_image(bytes) {
            Ok(loaded) => {
                let texture =
                    ctx.load_texture(&name, loaded.color_image, egui::TextureOptions::LINEAR);
                let [w, h] = loaded.size;
                self.map = Some(MapImage {
                    texture,
                    size: loaded.size,
                    bytes: loaded.bytes,
                });
                self.image_name = name;
                self.view.rotation = 0.0;
                self.pending_rotate = None;
                self.fit = Some(FitRequest::Map);
                self.status = match &georef {
                    Some(g) => format!("Loaded map ({w}x{h}) — georeferenced ({}).", g.describe()),
                    None => format!("Loaded map ({w}x{h})."),
                };
                self.georef = georef;
                self.refresh_georef_transform();
                self.recompute_all_transforms();
            }
            Err(e) => self.status = e,
        }
    }

    /// Parse a track file and add it as a new athlete (named after the file).
    pub(crate) fn add_athlete(&mut self, bytes: Vec<u8>, file_name: String) {
        match io::parse_track(&bytes) {
            Ok(track) => {
                if self.proj.is_none() {
                    self.proj = track
                        .centroid()
                        .map(|(la, lo)| LocalProjection::new(la, lo));
                }
                let projected = match &self.proj {
                    Some(p) => track
                        .points
                        .iter()
                        .map(|w| p.project(w.lat, w.lon))
                        .collect(),
                    None => Vec::new(),
                };
                let name = match file_name.rsplit_once('.') {
                    Some((stem, _)) if !stem.is_empty() => stem.to_string(),
                    _ => file_name.clone(),
                };
                self.status = format!(
                    "Added {name}: {} points, {:.2} km.",
                    track.len(),
                    track.total_distance() / 1000.0
                );
                let seg_metric = segment_metric(&track);
                let timeline = build_timeline(&track);
                self.athletes.push(Athlete {
                    name,
                    color: ATHLETE_COLORS[self.athletes.len() % ATHLETE_COLORS.len()],
                    visible: true,
                    track,
                    track_name: file_name,
                    track_bytes: bytes,
                    projected,
                    seg_metric,
                    calibration: Vec::new(),
                    transform: None,
                    matched: Vec::new(),
                    timeline,
                });
                self.active = self.athletes.len() - 1;
                // The shared projection may have just been anchored, and the georef
                // fit samples the tracks' extent — refresh before the transform so
                // a georeferenced map places this track immediately.
                self.refresh_georef_transform();
                self.recompute_transform_at(self.active);
                self.recompute_metric_active();
            }
            Err(e) => self.status = e,
        }
    }

    /// Refit the auto color range to the active athlete (manual cutoffs persist).
    pub(crate) fn recompute_metric_active(&mut self) {
        if let Some(a) = self.athletes.get_mut(self.active) {
            a.seg_metric = segment_metric(&a.track);
            if self.color_auto {
                self.metric_range = auto_range(&a.seg_metric);
            }
        }
    }

    /// The current pace color cutoffs as `(quick_end, slow_end)` in min/km.
    /// Pace is stored in sec/km, and a smaller value is quicker.
    pub(crate) fn friendly_cutoffs(&self) -> (f64, f64) {
        (self.metric_range.min / 60.0, self.metric_range.max / 60.0)
    }

    /// Set the pace color cutoffs from min/km and switch to manual mode.
    pub(crate) fn set_friendly_cutoffs(&mut self, quick_end: f64, slow_end: f64) {
        let (a, b) = (quick_end * 60.0, slow_end * 60.0);
        self.metric_range = MetricRange {
            min: a.min(b),
            max: a.max(b),
        };
        self.color_auto = false;
    }

    // --- Georeferencing ------------------------------------------------------

    pub(crate) fn recompute_transform_active(&mut self) {
        self.recompute_transform_at(self.active);
    }

    /// Rebuild athlete `i`'s meters->pixels transform from their locked calibration
    /// points so each one is honored exactly: 0 -> fallback (borrowed or bounding-box
    /// overlay), 1 -> translation on the fallback, 2 -> similarity, 3+ -> TPS.
    pub(crate) fn recompute_transform_at(&mut self, i: usize) {
        let t = self.compute_transform(i);
        if let Some(a) = self.athletes.get_mut(i) {
            a.transform = t;
        }
        self.rematch_athlete(i);
        self.refresh_px_to_m();
    }

    /// Recompute every athlete's transform, best-calibrated first so uncalibrated
    /// athletes can borrow a fresh transform.
    pub(crate) fn recompute_all_transforms(&mut self) {
        let mut order: Vec<usize> = (0..self.athletes.len()).collect();
        order.sort_by_key(|&i| std::cmp::Reverse(self.athletes[i].calibration.len()));
        for i in order {
            self.recompute_transform_at(i);
        }
    }

    fn compute_transform(&self, i: usize) -> Option<MapTransform> {
        let a = self.athletes.get(i)?;
        let pts: Vec<Correspondence> = a
            .calibration
            .iter()
            .filter_map(|c| {
                a.projected
                    .get(c.track_index)
                    .map(|&m| (m, (c.image_px[0], c.image_px[1])))
            })
            .collect();

        match pts.len() {
            0 => self.fallback_transform(i),
            1 => self.translation_transform(i, pts[0]),
            _ => MapTransform::fit(&pts).or_else(|| self.fallback_transform(i)),
        }
    }

    /// Base transform for an athlete with no usable fit of their own, in order of
    /// trust: the map's own georeferencing (survey-grade, unaffected by anyone's
    /// GPS drift), then the best-calibrated other athlete's transform (all
    /// transforms share one meter frame), then this athlete's bounding box fit
    /// into the map.
    fn fallback_transform(&self, i: usize) -> Option<MapTransform> {
        self.georef_mt
            .clone()
            .or_else(|| {
                self.athletes
                    .iter()
                    .enumerate()
                    .filter(|&(j, a)| j != i && !a.calibration.is_empty() && a.transform.is_some())
                    .max_by_key(|&(_, a)| a.calibration.len())
                    .and_then(|(_, a)| a.transform.clone())
            })
            .or_else(|| self.initial_transform(&self.athletes.get(i)?.projected))
    }

    /// Rebuild `georef_mt`: resolve the georef's CRS if needed (using the first
    /// track's centroid), then fit an affine meters→pixels transform by sampling
    /// the lat/lon → grid → pixel chain over the loaded tracks' extent. The chain
    /// is smooth and near-affine at map scale, so the fit residual is sub-pixel.
    pub(crate) fn refresh_georef_transform(&mut self) {
        self.georef_mt = None;
        let Some(map) = &self.map else { return };
        let (w, h) = (map.size[0] as f64, map.size[1] as f64);
        let Some(proj) = self.proj else { return };

        // Meter-space extent of all loaded tracks (the region the fit must serve).
        let (mut minx, mut maxx, mut miny, mut maxy) = (f64::MAX, f64::MIN, f64::MAX, f64::MIN);
        for a in &self.athletes {
            for &(x, y) in &a.projected {
                minx = minx.min(x);
                maxx = maxx.max(x);
                miny = miny.min(y);
                maxy = maxy.max(y);
            }
        }
        if minx > maxx {
            return;
        }
        let centroid = self.athletes.iter().find_map(|a| a.track.centroid());
        let Some(georef) = self.georef.as_mut() else {
            return;
        };
        let Some((clat, clon)) = centroid else { return };
        if !georef.resolve_crs(clat, clon, w, h) {
            return;
        }

        let dx = ((maxx - minx) * 0.1).max(50.0);
        let dy = ((maxy - miny) * 0.1).max(50.0);
        let (x0, x1, y0, y1) = (minx - dx, maxx + dx, miny - dy, maxy + dy);
        let mut pts: Vec<Correspondence> = Vec::with_capacity(16);
        for gy in 0..4 {
            for gx in 0..4 {
                let mx = x0 + (x1 - x0) * gx as f64 / 3.0;
                let my = y0 + (y1 - y0) * gy as f64 / 3.0;
                let (lat, lon) = proj.unproject(mx, my);
                if let Some(px) = georef.latlon_to_px(lat, lon) {
                    pts.push(((mx, my), px));
                }
            }
        }
        self.georef_mt = MapTransform::fit_affine(&pts);
        self.refresh_px_to_m();
    }

    /// Keep the fallback's scale/rotation but shift it so the single locked point
    /// lands exactly on its map feature.
    fn translation_transform(&self, i: usize, (meters, px): Correspondence) -> Option<MapTransform> {
        let base = self.fallback_transform(i)?;
        let mapped = base.apply(meters);
        let d = [px.0 - mapped.0, px.1 - mapped.1];
        Some(match base {
            MapTransform::Matrix(mut m) => {
                m[(0, 2)] += d[0];
                m[(1, 2)] += d[1];
                MapTransform::Matrix(m)
            }
            other => MapTransform::Translated(Box::new(other), d),
        })
    }

    /// A no-calibration starting overlay: fit the track's bounding box into the
    /// map image (north up) so the route is visible and ready to be pinned.
    fn initial_transform(&self, projected: &[(f64, f64)]) -> Option<MapTransform> {
        let map = self.map.as_ref()?;
        if projected.is_empty() {
            return None;
        }
        let (minx, maxx, miny, maxy) = projected.iter().fold(
            (f64::MAX, f64::MIN, f64::MAX, f64::MIN),
            |(minx, maxx, miny, maxy), &(x, y)| {
                (minx.min(x), maxx.max(x), miny.min(y), maxy.max(y))
            },
        );
        let (w, h) = (map.size[0] as f64, map.size[1] as f64);
        let margin = 0.1;
        let sx = (w * (1.0 - 2.0 * margin)) / (maxx - minx).max(1e-6);
        let sy = (h * (1.0 - 2.0 * margin)) / (maxy - miny).max(1e-6);
        let scale = sx.min(sy);
        let (cxm, cym) = ((minx + maxx) / 2.0, (miny + maxy) / 2.0);
        let (cx, cy) = (w / 2.0, h / 2.0);
        // Projected coords are already y-down, so both axes use +scale (no mirror):
        // u = scale*x + (cx - scale*cxm); v = scale*y + (cy - scale*cym)
        Some(MapTransform::Matrix(nalgebra::Matrix3::new(
            scale,
            0.0,
            cx - scale * cxm,
            0.0,
            scale,
            cy - scale * cym,
            0.0,
            0.0,
            1.0,
        )))
    }

    // --- Control matching ------------------------------------------------------

    /// Re-resolve which of athlete `i`'s waypoints passes each shared control.
    pub(crate) fn rematch_athlete(&mut self, i: usize) {
        let matched = match self.athletes.get(i) {
            Some(a) if !self.controls.is_empty() && a.transform.is_some() => {
                let t = a.transform.as_ref().unwrap();
                let route_px: Vec<(f64, f64)> =
                    a.projected.iter().map(|&m| t.apply(m)).collect();
                // Scale-aware radius, evaluated mid-route so a zoomed-in photo and a
                // whole-map scan both get a sensible on-the-ground tolerance.
                let mid = a.projected.get(a.projected.len() / 2).copied();
                let radius = mid
                    .map(|m| (MATCH_RADIUS_M * local_scale_px_per_m(t, m)).clamp(20.0, 300.0))
                    .unwrap_or(60.0);
                let controls: Vec<[f64; 2]> = self.controls.iter().map(|c| c.image_px).collect();
                match_controls(&route_px, &controls, radius)
            }
            Some(_) => vec![None; self.controls.len()],
            None => return,
        };
        if let Some(a) = self.athletes.get_mut(i) {
            a.matched = matched;
        }
    }

    /// Re-resolve control matches for every athlete (after any course edit).
    pub(crate) fn rematch_all(&mut self) {
        for i in 0..self.athletes.len() {
            self.rematch_athlete(i);
        }
        self.clamp_selected_leg();
        // A course edit can orphan a leg-attached route and moves control
        // positions, so re-derive route attachments and stats.
        self.reconcile_drawn_routes();
        self.recompute_drawn_stats();
    }

    // --- Leg view --------------------------------------------------------------

    /// Number of legs on the course (start→1, …, n→finish).
    pub(crate) fn n_legs(&self) -> usize {
        self.controls.len() + 1
    }

    /// Drop the leg selection if it no longer addresses a real leg (e.g. controls
    /// were removed).
    pub(crate) fn clamp_selected_leg(&mut self) {
        if let Some(li) = self.selected_leg
            && li >= self.n_legs()
        {
            self.selected_leg = None;
        }
    }

    /// Select a leg for the on-map leg view (or `None` for the full course), and
    /// request a view fit to that leg. Also resets the replay clock so playback
    /// restarts at the new leg's start.
    pub(crate) fn select_leg(&mut self, li: Option<usize>) {
        self.selected_leg = li.filter(|&li| li < self.n_legs());
        self.playback.clock = 0.0;
        match self.selected_leg {
            Some(li) => {
                if let Some((min, max)) = self.leg_bbox(li) {
                    self.fit = Some(FitRequest::Rect { min, max });
                }
            }
            None => self.fit = Some(FitRequest::Map),
        }
    }

    /// Image-space bounding box of leg `li`: the union of every visible athlete's
    /// route choice for that leg (where both boundaries matched) plus the leg's two
    /// controls. `None` if nothing can be located.
    fn leg_bbox(&self, li: usize) -> Option<((f64, f64), (f64, f64))> {
        let mut min = (f64::MAX, f64::MAX);
        let mut max = (f64::MIN, f64::MIN);
        let mut grow = |(x, y): (f64, f64)| {
            min.0 = min.0.min(x);
            min.1 = min.1.min(y);
            max.0 = max.0.max(x);
            max.1 = max.1.max(y);
        };
        // The leg's endpoint controls (control li-1 .. li in course numbering; for
        // the start/finish legs one endpoint is an athlete's S/F instead).
        if li >= 1
            && let Some(c) = self.controls.get(li - 1)
        {
            grow((c.image_px[0], c.image_px[1]));
        }
        if li < self.controls.len()
            && let Some(c) = self.controls.get(li)
        {
            grow((c.image_px[0], c.image_px[1]));
        }
        // Each visible athlete's route segments for this leg.
        for a in self.athletes.iter().filter(|a| a.visible) {
            let Some(t) = &a.transform else { continue };
            let b = a.boundaries();
            let (Some(from), Some(to)) = (
                b.get(li).copied().flatten(),
                b.get(li + 1).copied().flatten(),
            ) else {
                continue;
            };
            for &m in &a.projected[from..=to.min(a.projected.len().saturating_sub(1))] {
                grow(t.apply(m));
            }
        }
        (min.0 <= max.0).then_some((min, max))
    }

    // --- Replay ----------------------------------------------------------------

    /// Athlete indices to animate, in draw order (non-active first, active last).
    /// Solo mode animates only the active athlete.
    pub(crate) fn animated_indices(&self) -> Vec<usize> {
        if self.playback.solo {
            return if self.athletes.is_empty() {
                Vec::new()
            } else {
                vec![self.active]
            };
        }
        let mut v: Vec<usize> = (0..self.athletes.len())
            .filter(|&i| i != self.active && self.athletes[i].visible)
            .collect();
        if self.active().is_some_and(|a| a.visible) {
            v.push(self.active);
        }
        v
    }

    /// The replay clock mode: a leg-restart when a leg is selected, otherwise the
    /// user's chosen start mode.
    pub(crate) fn playback_clock_mode(&self) -> ClockMode {
        match self.selected_leg {
            Some(li) => ClockMode::Leg(li),
            None => match self.playback.mode {
                StartMode::MassStart => ClockMode::MassStart,
                StartMode::RealTime => ClockMode::RealTime,
            },
        }
    }

    /// Real-time anchor: the earliest start among animated athletes.
    pub(crate) fn playback_anchor(&self) -> Option<DateTime<Utc>> {
        self.animated_indices()
            .iter()
            .filter_map(|&i| self.athletes[i].start_time())
            .min()
    }

    /// Athlete `i`'s playback window under the current mode/leg/anchor.
    pub(crate) fn window_for(&self, i: usize, anchor: Option<DateTime<Utc>>) -> Option<Window> {
        let a = self.athletes.get(i)?;
        let boundaries = a.boundaries();
        playback::window(
            &a.timeline,
            &boundaries,
            self.playback_clock_mode(),
            a.start_time(),
            anchor,
        )
    }

    /// Total length of the replay timeline (seconds), across animated athletes.
    pub(crate) fn playback_total(&self) -> f64 {
        let anchor = self.playback_anchor();
        total_span(
            self.animated_indices()
                .iter()
                .filter_map(|&i| self.window_for(i, anchor)),
        )
    }

    // --- Coordinate helpers --------------------------------------------------

    pub(crate) fn to_screen(&self, origin: Pos2, img: (f64, f64)) -> Pos2 {
        let (sin, cos) = self.view.rotation.sin_cos();
        let x = img.0 as f32 * self.view.zoom;
        let y = img.1 as f32 * self.view.zoom;
        pos2(
            origin.x + self.view.offset[0] + cos * x - sin * y,
            origin.y + self.view.offset[1] + sin * x + cos * y,
        )
    }

    pub(crate) fn to_image(&self, origin: Pos2, s: Pos2) -> (f64, f64) {
        let (sin, cos) = self.view.rotation.sin_cos();
        let dx = s.x - origin.x - self.view.offset[0];
        let dy = s.y - origin.y - self.view.offset[1];
        // Undo the rotation (transpose), then the zoom.
        (
            ((cos * dx + sin * dy) / self.view.zoom) as f64,
            ((-sin * dx + cos * dy) / self.view.zoom) as f64,
        )
    }

    /// Request a new absolute view rotation (radians, clockwise), applied next
    /// frame about the canvas center once the canvas rect is known.
    pub(crate) fn rotate_to(&mut self, angle: f32) {
        self.pending_rotate = Some(angle);
    }

    /// Request a relative view rotation, stacking on any rotation already pending
    /// this frame (so repeated 90° taps accumulate).
    pub(crate) fn rotate_by(&mut self, delta: f32) {
        let base = self.pending_rotate.unwrap_or(self.view.rotation);
        self.pending_rotate = Some(base + delta);
    }

    /// Screen position of the active athlete's waypoint `i`, if a transform exists.
    pub(crate) fn waypoint_screen(&self, origin: Pos2, i: usize) -> Option<Pos2> {
        let a = self.active()?;
        let t = a.transform.as_ref()?;
        let m = *a.projected.get(i)?;
        Some(self.to_screen(origin, t.apply(m)))
    }

    /// Active athlete's waypoint index nearest a given along-track distance (km).
    pub(crate) fn track_index_at_km(&self, km: f64) -> Option<usize> {
        let track = &self.active()?.track;
        if track.is_empty() {
            return None;
        }
        let target = km * 1000.0;
        track
            .cumulative_distance()
            .iter()
            .enumerate()
            .min_by(|a, b| (a.1 - target).abs().total_cmp(&(b.1 - target).abs()))
            .map(|(i, _)| i)
    }

    /// Along-track distance (km) at an active-athlete waypoint index.
    pub(crate) fn km_at_index(&self, i: usize) -> Option<f64> {
        let track = &self.active()?.track;
        track.cumulative_distance().get(i).map(|&d| d / 1000.0)
    }

    /// Map a WGS84 position to image pixels: through the map's georeferencing
    /// when available, else through the best-calibrated athlete's transform.
    pub(crate) fn latlon_to_px(&self, lat: f64, lon: f64) -> Option<(f64, f64)> {
        if let Some(g) = &self.georef
            && let Some(px) = g.latlon_to_px(lat, lon)
        {
            return Some(px);
        }
        let proj = self.proj?;
        let m = proj.project(lat, lon);
        let t = self
            .athletes
            .iter()
            .filter(|a| !a.calibration.is_empty())
            .max_by_key(|a| a.calibration.len())?
            .transform
            .as_ref()?;
        Some(t.apply(m))
    }

    // --- Drawn route options (analysis board) --------------------------------

    /// Meter-space bounding box of all loaded tracks — the region the pixel→meters
    /// inverse must serve. `None` when nothing is projected yet.
    fn tracks_bounds_m(&self) -> Option<((f64, f64), (f64, f64))> {
        let (mut minx, mut maxx, mut miny, mut maxy) = (f64::MAX, f64::MIN, f64::MAX, f64::MIN);
        for a in &self.athletes {
            for &(x, y) in &a.projected {
                minx = minx.min(x);
                maxx = maxx.max(x);
                miny = miny.min(y);
                maxy = maxy.max(y);
            }
        }
        // Pad so a degenerate (single-track-point) extent still has area.
        (minx <= maxx).then(|| {
            let dx = ((maxx - minx) * 0.1).max(50.0);
            let dy = ((maxy - miny) * 0.1).max(50.0);
            ((minx - dx, miny - dy), (maxx + dx, maxy + dy))
        })
    }

    /// Rebuild the cached pixels→meters transform from the best-calibrated
    /// athlete's map transform (needs ≥2 pins for a real ground scale), then
    /// refresh the drawn-route stats that depend on it. The georef path is applied
    /// per-point in `px_polyline_len_m`, so it isn't cached here.
    pub(crate) fn refresh_px_to_m(&mut self) {
        self.px_to_m = self.tracks_bounds_m().and_then(|bounds| {
            let t = self
                .athletes
                .iter()
                .filter(|a| a.calibration.len() >= 2 && a.transform.is_some())
                .max_by_key(|a| a.calibration.len())?
                .transform
                .as_ref()?;
            invert_transform(t, bounds)
        });
        self.recompute_drawn_stats();
    }

    /// Ground length of a pixel-space polyline in meters, or `None` if the map
    /// can't be measured yet. Prefers the map's georeferencing, then the inverse
    /// of a calibrated athlete's transform.
    pub(crate) fn px_polyline_len_m(&self, pts: &[[f64; 2]]) -> Option<f64> {
        if pts.len() < 2 {
            return Some(0.0);
        }
        if let Some(g) = &self.georef {
            match g.crs {
                Crs::TransverseMercator(_) => {
                    // Grid meters: straight Euclidean sum in world coordinates.
                    return Some(
                        pts.windows(2)
                            .map(|w| {
                                let a = g.px_to_world(w[0][0], w[0][1]);
                                let b = g.px_to_world(w[1][0], w[1][1]);
                                ((b.0 - a.0).powi(2) + (b.1 - a.1).powi(2)).sqrt()
                            })
                            .sum(),
                    );
                }
                Crs::Geographic => {
                    // World coords are lon/lat: sum great-circle segments.
                    return Some(
                        pts.windows(2)
                            .map(|w| {
                                let a = g.px_to_world(w[0][0], w[0][1]);
                                let b = g.px_to_world(w[1][0], w[1][1]);
                                haversine(&ll(a.1, a.0), &ll(b.1, b.0))
                            })
                            .sum(),
                    );
                }
                Crs::UnknownProjected => {} // fall through to the calibration path
            }
        }
        let t = self.px_to_m.as_ref()?;
        Some(
            pts.windows(2)
                .map(|w| {
                    let a = t.apply((w[0][0], w[0][1]));
                    let b = t.apply((w[1][0], w[1][1]));
                    ((b.0 - a.0).powi(2) + (b.1 - a.1).powi(2)).sqrt()
                })
                .sum(),
        )
    }

    /// Pixels per ground meter near an image point, for scaling the control
    /// collection radius. `None` when the map can't be measured.
    fn px_per_m_at(&self, px: (f64, f64)) -> Option<f64> {
        if let Some(g) = &self.georef
            && matches!(g.crs, Crs::TransverseMercator(_))
        {
            let o = g.px_to_world(px.0, px.1);
            let ex = g.px_to_world(px.0 + 1.0, px.1);
            let ey = g.px_to_world(px.0, px.1 + 1.0);
            let mx = ((ex.0 - o.0).powi(2) + (ex.1 - o.1).powi(2)).sqrt();
            let my = ((ey.0 - o.0).powi(2) + (ey.1 - o.1).powi(2)).sqrt();
            let m_per_px = (mx + my) / 2.0;
            if m_per_px > 1e-9 {
                return Some(1.0 / m_per_px);
            }
        }
        // `px_to_m` maps pixels→meters, so its local scale is meters per pixel.
        let t = self.px_to_m.as_ref()?;
        let m_per_px = local_scale_px_per_m(t, px);
        (m_per_px > 1e-9).then_some(1.0 / m_per_px)
    }

    /// Rebuild every drawn route's derived stats (length, collected controls,
    /// points). Cheap for the handful of routes a user draws.
    pub(crate) fn recompute_drawn_stats(&mut self) {
        let controls: Vec<[f64; 2]> = self.controls.iter().map(|c| c.image_px).collect();
        self.drawn_stats = self
            .drawn_routes
            .iter()
            .map(|r| {
                let length_m = self.px_polyline_len_m(&r.points);
                let radius = route_midpoint_px(&r.points)
                    .and_then(|m| self.px_per_m_at((m[0], m[1])))
                    .map(|ppm| (MATCH_RADIUS_M * ppm).clamp(20.0, 300.0))
                    .unwrap_or(40.0);
                let collected = collected_controls(&r.points, &controls, radius);
                let points = collected
                    .iter()
                    .filter_map(|&i| self.controls.get(i).and_then(|c| c.score))
                    .sum();
                RouteStats {
                    length_m,
                    collected,
                    points,
                }
            })
            .collect();
    }

    /// Detach any drawn route whose leg no longer exists (controls were removed),
    /// degrading it to a free-form route rather than dropping it.
    pub(crate) fn reconcile_drawn_routes(&mut self) {
        let n = self.n_legs();
        for r in &mut self.drawn_routes {
            if r.leg.is_some_and(|li| li >= n) {
                r.leg = None;
            }
        }
    }

    /// Image-space bounding box of a drawn route, to frame it on select.
    pub(crate) fn route_bbox(&self, i: usize) -> Option<((f64, f64), (f64, f64))> {
        let r = self.drawn_routes.get(i)?;
        if r.points.is_empty() {
            return None;
        }
        let (mut min, mut max) = ((f64::MAX, f64::MAX), (f64::MIN, f64::MIN));
        for p in &r.points {
            min.0 = min.0.min(p[0]);
            min.1 = min.1.min(p[1]);
            max.0 = max.0.max(p[0]);
            max.1 = max.1.max(p[1]);
        }
        Some((min, max))
    }

    /// Import an IOF XML 3.0 course: place its controls on the map (replacing the
    /// current course) using the file's geo positions.
    pub(crate) fn import_course(&mut self, bytes: &[u8]) {
        let course = match io::parse_iof_course(bytes) {
            Ok(c) => c,
            Err(e) => {
                self.status = format!("Course import failed: {e}");
                return;
            }
        };
        let pts: Vec<CoursePoint> = course
            .controls
            .iter()
            .filter_map(|&(lat, lon)| self.latlon_to_px(lat, lon))
            .map(|(x, y)| CoursePoint::at(x, y))
            .collect();
        if pts.is_empty() {
            self.status = "Can't place the course yet: open a georeferenced map, or calibrate \
                           a track first."
                .into();
            return;
        }
        let replaced = !self.controls.is_empty();
        let n = pts.len();
        self.controls = pts;
        self.rematch_all();

        let mut s = match &course.course_name {
            Some(name) => format!("Imported course \"{name}\": {n} controls"),
            None => format!("Imported {n} controls"),
        };
        if course.n_courses > 1 {
            s += &format!(" (first of {} courses in the file)", course.n_courses);
        }
        if course.skipped > 0 {
            s += &format!("; {} skipped without a geo position", course.skipped);
        }
        if replaced {
            s += "; replaced the previous course";
        }
        s += ".";
        self.status = s;
    }

    /// Index of the active athlete's waypoint nearest to a screen point.
    pub(crate) fn nearest_waypoint(&self, origin: Pos2, p: Pos2) -> Option<usize> {
        let a = self.active()?;
        let t = a.transform.as_ref()?;
        a.projected
            .iter()
            .enumerate()
            .map(|(i, &m)| {
                let s = self.to_screen(origin, t.apply(m));
                (i, (s - p).length_sq())
            })
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(i, _)| i)
    }

    // --- Persistence ---------------------------------------------------------

    pub(crate) fn to_project_file(&self) -> ProjectFileV2 {
        ProjectFileV2 {
            version: 2,
            image_name: self.image_name.clone(),
            athletes: self
                .athletes
                .iter()
                .enumerate()
                .map(|(i, a)| AthleteFile {
                    name: a.name.clone(),
                    color: [a.color.r(), a.color.g(), a.color.b()],
                    visible: a.visible,
                    // Per-athlete folders keep same-named track files from colliding.
                    track_entry: format!("tracks/{i}/{}", a.track_name),
                    calibration: a.calibration.clone(),
                })
                .collect(),
            controls: self.controls.clone(),
            active: self.active,
            view: self.view,
            georef: self.georef.as_ref().map(|g| g.to_file()),
            routes: self.drawn_routes.clone(),
        }
    }

    /// Serialize the current project to `.legit` bytes, or an error message.
    fn project_bytes(&self) -> Result<Vec<u8>, String> {
        let Some(map) = &self.map else {
            return Err("Nothing to save yet.".into());
        };
        let bundle = ProjectBundle {
            project: self.to_project_file(),
            image_bytes: map.bytes.clone(),
            tracks: self.athletes.iter().map(|a| a.track_bytes.clone()).collect(),
            legacy_control_indices: None,
        };
        io::write_bundle(&bundle)
    }

    /// Save the project: serialize, then hand the bytes to the platform save
    /// (native dialog + write, or web download).
    pub(crate) fn save_project(&mut self) {
        let bytes = match self.project_bytes() {
            Ok(b) => b,
            Err(e) => {
                self.status = e;
                return;
            }
        };
        let name = self
            .project_path
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "analysis.legit".into());
        match platform::save_file(SaveKind::Project, &name, bytes) {
            Ok(true) => self.status = "Project saved.".into(),
            Ok(false) => {} // cancelled
            Err(e) => self.status = format!("Save failed: {e}"),
        }
    }

    /// Load a project from `.legit` bytes. `path` (native only) lets us remember a
    /// suggested name for the next save.
    pub(crate) fn open_project_bytes(
        &mut self,
        ctx: &egui::Context,
        bytes: Vec<u8>,
        path: Option<PathBuf>,
    ) {
        let bundle = match io::read_bundle(&bytes) {
            Ok(b) => b,
            Err(e) => {
                self.status = format!("Open failed: {e}");
                return;
            }
        };
        let project = bundle.project;

        self.athletes.clear();
        self.proj = None;
        self.controls.clear();
        let georef = project.georef.as_ref().map(MapGeoref::from_file);
        self.load_image_from_bytes(ctx, bundle.image_bytes, project.image_name.clone(), georef);
        for (meta, track_bytes) in project.athletes.into_iter().zip(bundle.tracks) {
            let file_name = meta
                .track_entry
                .rsplit('/')
                .next()
                .unwrap_or(&meta.track_entry)
                .to_string();
            let before = self.athletes.len();
            self.add_athlete(track_bytes, file_name);
            if self.athletes.len() > before
                && let Some(a) = self.athletes.last_mut()
            {
                a.name = meta.name;
                a.color = Color32::from_rgb(meta.color[0], meta.color[1], meta.color[2]);
                a.visible = meta.visible;
                a.calibration = meta.calibration;
            }
        }
        self.active = project
            .active
            .min(self.athletes.len().saturating_sub(1));
        self.controls = project.controls;
        self.drawn_routes = project.routes;
        self.selected_route = None;
        self.draft = None;
        self.draw_mode = false;
        self.recompute_all_transforms();

        // V1 controls were waypoint indices into the single track; place them on the
        // map exactly where the old renderer drew them (through the track's transform).
        if let Some(indices) = bundle.legacy_control_indices
            && let Some(a) = self.athletes.first()
            && let Some(t) = &a.transform
        {
            self.controls = indices
                .iter()
                .filter_map(|&i| a.projected.get(i))
                .map(|&m| {
                    let (x, y) = t.apply(m);
                    CoursePoint::at(x, y)
                })
                .collect();
        }
        self.rematch_all();
        self.recompute_metric_active();
        self.view = project.view;
        self.fit = None;
        self.selected_leg = None;
        // A saved project is already set up — land straight in Analysis.
        self.tab = ViewTab::Analysis;
        self.project_path = path;
        self.status = "Project opened.".into();
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        // Deliver any files the pickers resolved since last frame (collect first so
        // the receiver borrow ends before `deliver_file` takes `&mut self`).
        let ctx = ui.ctx().clone();
        for f in self.file_rx.try_iter().collect::<Vec<_>>() {
            self.deliver_file(&ctx, f);
        }
        // Screen class + touch: a narrow window (phone) gets the map-first mobile
        // layout; the touch flag latches on the first touch and never clears.
        // The root Ui spans the whole window before any panels take their space.
        self.screen = if ui.max_rect().width() < 600.0 {
            ScreenClass::Narrow
        } else {
            ScreenClass::Wide
        };
        if ctx.input(|i| i.any_touches()) {
            self.touch = true;
        }
        if self.touch {
            Self::apply_touch_style(&ctx);
        }

        // Panels draw the cross-highlight cursor from last frame's `hover_km` and
        // report this frame's hover into `pending_hover`; commit it at the end.
        self.pending_hover = None;
        match self.screen {
            ScreenClass::Wide => {
                self.top_bar(ui);
                self.side_panel(ui);
                match self.tab {
                    ViewTab::Setup => self.map_panel(ui),
                    ViewTab::Analysis => {
                        // Bottom stack, outermost first: transport bar, then the
                        // Splits and Graphs drawers above it, map filling the rest.
                        self.playback_bar(ui);
                        self.splits_drawer(ui);
                        self.bottom_graphs(ui);
                        self.map_panel(ui);
                    }
                }
            }
            ScreenClass::Narrow => self.mobile_ui(ui),
        }
        self.hover_km = self.pending_hover;
        self.hover_index = self.hover_km.and_then(|km| self.track_index_at_km(km));
        if self.hover_km.is_some() {
            ui.ctx().request_repaint(); // settle the 1-frame lag while hovering
        }
    }
}
