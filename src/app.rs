use crate::analysis::{
    ClockMode, MetricRange, Window, auto_range, build_timeline, local_scale_px_per_m,
    match_controls, playback, segment_metric, total_span,
};
use crate::athlete::{ATHLETE_COLORS, Athlete};
use crate::geo::{Correspondence, LocalProjection, MapTransform};
use crate::io;
use crate::io::ProjectBundle;
use crate::model::{AthleteFile, CoursePoint, ProjectFileV2, ViewState};
use chrono::{DateTime, Utc};
use egui::{Color32, Pos2, TextureHandle, pos2};
use std::path::PathBuf;

/// A decoded map image plus its GPU texture and original encoded bytes.
pub struct MapImage {
    pub texture: TextureHandle,
    pub size: [usize; 2],
    pub bytes: Vec<u8>,
}

/// The top-level view: the map canvas or the leg-by-leg comparison.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ViewTab {
    Map,
    LegAnalysis,
}

/// What a canvas click/drag currently does. (Dragging empty space always pans.)
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum EditMode {
    Calibrate,
    Control,
}

/// What the active pointer drag is manipulating.
#[derive(Clone, Copy)]
pub enum DragTarget {
    View,
    Calibration(usize),
    Control(usize),
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
    /// The shared course: controls placed on the map, in course order.
    pub(crate) controls: Vec<CoursePoint>,
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
    pub(crate) show_pace: bool,
    pub(crate) show_hr: bool,
    pub(crate) show_ele: bool,
    /// Draw the active athlete's route with pace coloring instead of its solid color.
    pub(crate) active_pace_colors: bool,
    /// Show cumulative times in the leg comparison table.
    pub(crate) show_cumulative: bool,
    /// A pending view fit, applied next frame; `None` at rest.
    pub(crate) fit: Option<FitRequest>,
    /// Selected leg for the on-map leg view (0-based leg index), or `None` for
    /// the whole-course view. Leg `li` runs from boundary `li` to `li + 1`.
    pub(crate) selected_leg: Option<usize>,
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

    // UI
    pub(crate) status: String,
}

impl App {
    pub fn new(_cc: &eframe::CreationContext<'_>) -> Self {
        Self {
            map: None,
            athletes: Vec::new(),
            active: 0,
            proj: None,
            controls: Vec::new(),
            metric_range: MetricRange { min: 0.0, max: 1.0 },
            color_auto: true,
            palette_view: None,
            pace_cap_minkm: None,
            tab: ViewTab::Map,
            view: ViewState::default(),
            mode: EditMode::Calibrate,
            drag: None,
            show_pace: true,
            show_hr: true,
            show_ele: true,
            active_pace_colors: true,
            show_cumulative: true,
            fit: None,
            selected_leg: None,
            playback: Playback::default(),
            hover_km: None,
            hover_index: None,
            pending_hover: None,
            image_name: String::new(),
            project_path: None,
            status: "Open a map image and add a GPS track to begin.".into(),
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
        self.recompute_metric_active();
    }

    // --- Loading -------------------------------------------------------------

    pub(crate) fn load_image_from_bytes(
        &mut self,
        ctx: &egui::Context,
        bytes: Vec<u8>,
        name: String,
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
                self.fit = Some(FitRequest::Map);
                self.recompute_all_transforms();
                self.status = format!("Loaded map ({w}x{h}).");
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

    /// Base transform for an athlete with no usable fit of their own: borrow the
    /// transform of the best-calibrated other athlete (all transforms share one
    /// meter frame), else fit this athlete's bounding box into the map.
    fn fallback_transform(&self, i: usize) -> Option<MapTransform> {
        self.athletes
            .iter()
            .enumerate()
            .filter(|&(j, a)| j != i && !a.calibration.is_empty() && a.transform.is_some())
            .max_by_key(|&(_, a)| a.calibration.len())
            .and_then(|(_, a)| a.transform.clone())
            .or_else(|| self.initial_transform(&self.athletes.get(i)?.projected))
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
        pos2(
            origin.x + self.view.offset[0] + img.0 as f32 * self.view.zoom,
            origin.y + self.view.offset[1] + img.1 as f32 * self.view.zoom,
        )
    }

    pub(crate) fn to_image(&self, origin: Pos2, s: Pos2) -> (f64, f64) {
        (
            ((s.x - origin.x - self.view.offset[0]) / self.view.zoom) as f64,
            ((s.y - origin.y - self.view.offset[1]) / self.view.zoom) as f64,
        )
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
        }
    }

    pub(crate) fn save_project(&mut self, path: PathBuf) {
        let Some(map) = &self.map else {
            self.status = "Nothing to save yet.".into();
            return;
        };
        let bundle = ProjectBundle {
            project: self.to_project_file(),
            image_bytes: map.bytes.clone(),
            tracks: self.athletes.iter().map(|a| a.track_bytes.clone()).collect(),
            legacy_control_indices: None,
        };
        match io::write_bundle(&bundle)
            .and_then(|b| std::fs::write(&path, b).map_err(|e| e.to_string()))
        {
            Ok(()) => {
                self.project_path = Some(path);
                self.status = "Project saved.".into();
            }
            Err(e) => self.status = format!("Save failed: {e}"),
        }
    }

    pub(crate) fn open_project(&mut self, ctx: &egui::Context, path: PathBuf) {
        let bytes = match std::fs::read(&path) {
            Ok(b) => b,
            Err(e) => {
                self.status = format!("Open failed: {e}");
                return;
            }
        };
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
        self.load_image_from_bytes(ctx, bundle.image_bytes, project.image_name.clone());
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
                    CoursePoint { image_px: [x, y] }
                })
                .collect();
        }
        self.rematch_all();
        self.recompute_metric_active();
        self.view = project.view;
        self.fit = None;
        self.selected_leg = None;
        self.project_path = Some(path);
        self.status = "Project opened.".into();
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        // Panels draw the cross-highlight cursor from last frame's `hover_km` and
        // report this frame's hover into `pending_hover`; commit it at the end.
        self.pending_hover = None;
        self.top_bar(ui);
        self.side_panel(ui);
        match self.tab {
            ViewTab::Map => {
                self.playback_bar(ui);
                self.bottom_graphs(ui);
                self.map_panel(ui);
            }
            ViewTab::LegAnalysis => self.leg_analysis_panel(ui),
        }
        self.hover_km = self.pending_hover;
        self.hover_index = self.hover_km.and_then(|km| self.track_index_at_km(km));
        if self.hover_km.is_some() {
            ui.ctx().request_repaint(); // settle the 1-frame lag while hovering
        }
    }
}
