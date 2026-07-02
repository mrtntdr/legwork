use crate::analysis::{MetricRange, auto_range, segment_metric};
use crate::geo::{Correspondence, LocalProjection, MapTransform};
use crate::io;
use crate::io::ProjectBundle;
use crate::model::{CalibrationPoint, Control, ProjectFile, Track, ViewState};
use egui::{Pos2, TextureHandle, pos2};
use std::path::PathBuf;

/// A decoded map image plus its GPU texture and original encoded bytes.
pub struct MapImage {
    pub texture: TextureHandle,
    pub size: [usize; 2],
    pub bytes: Vec<u8>,
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
}

pub struct App {
    // Loaded data
    pub(crate) map: Option<MapImage>,
    pub(crate) track: Option<Track>,
    pub(crate) proj: Option<LocalProjection>,
    pub(crate) projected: Vec<(f64, f64)>,
    pub(crate) seg_metric: Vec<f64>,
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
    pub(crate) calibration: Vec<CalibrationPoint>,
    pub(crate) controls: Vec<usize>,
    pub(crate) transform: Option<MapTransform>,

    // View + interaction
    pub(crate) view: ViewState,
    pub(crate) mode: EditMode,
    pub(crate) drag: Option<DragTarget>,
    pub(crate) show_pace: bool,
    pub(crate) show_hr: bool,
    pub(crate) show_ele: bool,
    pub(crate) fit_requested: bool,

    /// Cross-highlight between the graphs and the route: the along-track position
    /// (km) currently pointed at, its resolved waypoint index, and this frame's
    /// freshly-detected hover (committed to `hover_km` at end of frame).
    pub(crate) hover_km: Option<f64>,
    pub(crate) hover_index: Option<usize>,
    pub(crate) pending_hover: Option<f64>,

    // Files
    pub(crate) image_name: String,
    pub(crate) track_name: String,
    pub(crate) track_bytes: Vec<u8>,
    pub(crate) project_path: Option<PathBuf>,

    // UI
    pub(crate) status: String,
}

impl App {
    pub fn new(_cc: &eframe::CreationContext<'_>) -> Self {
        Self {
            map: None,
            track: None,
            proj: None,
            projected: Vec::new(),
            seg_metric: Vec::new(),
            metric_range: MetricRange { min: 0.0, max: 1.0 },
            color_auto: true,
            palette_view: None,
            pace_cap_minkm: None,
            calibration: Vec::new(),
            controls: Vec::new(),
            transform: None,
            view: ViewState::default(),
            mode: EditMode::Calibrate,
            drag: None,
            show_pace: true,
            show_hr: true,
            show_ele: true,
            fit_requested: false,
            hover_km: None,
            hover_index: None,
            pending_hover: None,
            image_name: String::new(),
            track_name: String::new(),
            track_bytes: Vec::new(),
            project_path: None,
            status: "Open a map image and a GPS track to begin.".into(),
        }
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
                self.fit_requested = true;
                self.recompute_transform();
                self.status = format!("Loaded map ({w}x{h}).");
            }
            Err(e) => self.status = e,
        }
    }

    pub(crate) fn load_track_from_bytes(&mut self, bytes: Vec<u8>, name: String) {
        match io::parse_track(&bytes) {
            Ok(track) => {
                self.track_bytes = bytes;
                self.track_name = name;
                self.status = format!(
                    "Loaded track: {} points, {:.2} km.",
                    track.len(),
                    track.total_distance() / 1000.0
                );
                self.set_track(track);
            }
            Err(e) => self.status = e,
        }
    }

    fn set_track(&mut self, track: Track) {
        self.proj = track
            .centroid()
            .map(|(la, lo)| LocalProjection::new(la, lo));
        self.projected = match &self.proj {
            Some(p) => track
                .points
                .iter()
                .map(|w| p.project(w.lat, w.lon))
                .collect(),
            None => Vec::new(),
        };
        self.calibration.clear();
        self.controls.clear();
        self.pace_cap_minkm = None;
        self.recompute_metric(&track);
        self.track = Some(track);
        self.recompute_transform();
    }

    pub(crate) fn recompute_metric_current(&mut self) {
        if let Some(track) = self.track.take() {
            self.recompute_metric(&track);
            self.track = Some(track);
        }
    }

    fn recompute_metric(&mut self, track: &Track) {
        self.seg_metric = segment_metric(track);
        if self.color_auto {
            self.metric_range = auto_range(&self.seg_metric);
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

    /// Rebuild the meters->pixels transform from the locked calibration points so
    /// each one is honored exactly: 0 -> bounding-box overlay, 1 -> translation,
    /// 2 -> similarity, 3+ -> interpolating TPS.
    pub(crate) fn recompute_transform(&mut self) {
        if self.track.is_none() {
            self.transform = None;
            return;
        }
        let pts: Vec<Correspondence> = self
            .calibration
            .iter()
            .filter_map(|c| {
                self.projected
                    .get(c.track_index)
                    .map(|&m| (m, (c.image_px[0], c.image_px[1])))
            })
            .collect();

        self.transform = match pts.len() {
            0 => self.initial_transform(),
            1 => self.translation_transform(pts[0]),
            _ => MapTransform::fit(&pts).or_else(|| self.initial_transform()),
        };
    }

    /// Keep the initial overlay's scale/rotation but shift it so the single locked
    /// point lands exactly on its map feature.
    fn translation_transform(&self, (meters, px): Correspondence) -> Option<MapTransform> {
        let base = self.initial_transform()?;
        let MapTransform::Matrix(mut m) = base else {
            return Some(base);
        };
        let mapped = MapTransform::Matrix(m).apply(meters);
        m[(0, 2)] += px.0 - mapped.0;
        m[(1, 2)] += px.1 - mapped.1;
        Some(MapTransform::Matrix(m))
    }

    /// A no-calibration starting overlay: fit the track's bounding box into the
    /// map image (north up) so the route is visible and ready to be pinned.
    fn initial_transform(&self) -> Option<MapTransform> {
        let map = self.map.as_ref()?;
        if self.projected.is_empty() {
            return None;
        }
        let (minx, maxx, miny, maxy) = self.projected.iter().fold(
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

    /// Screen position of waypoint `i`, if a transform exists.
    pub(crate) fn waypoint_screen(&self, origin: Pos2, i: usize) -> Option<Pos2> {
        let t = self.transform.as_ref()?;
        let m = *self.projected.get(i)?;
        Some(self.to_screen(origin, t.apply(m)))
    }

    /// Waypoint index nearest a given along-track distance (km).
    pub(crate) fn track_index_at_km(&self, km: f64) -> Option<usize> {
        let track = self.track.as_ref()?;
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

    /// Along-track distance (km) at a waypoint index.
    pub(crate) fn km_at_index(&self, i: usize) -> Option<f64> {
        let track = self.track.as_ref()?;
        track.cumulative_distance().get(i).map(|&d| d / 1000.0)
    }

    /// Index of the route waypoint nearest to a screen point (within reason).
    pub(crate) fn nearest_waypoint(&self, origin: Pos2, p: Pos2) -> Option<usize> {
        let t = self.transform.as_ref()?;
        self.projected
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

    pub(crate) fn to_project_file(&self) -> ProjectFile {
        ProjectFile {
            image_name: self.image_name.clone(),
            track_name: self.track_name.clone(),
            calibration: self.calibration.clone(),
            controls: self
                .controls
                .iter()
                .map(|&track_index| Control { track_index })
                .collect(),
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
            track_bytes: self.track_bytes.clone(),
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
        self.load_track_from_bytes(bundle.track_bytes, bundle.project.track_name.clone());
        self.load_image_from_bytes(ctx, bundle.image_bytes, bundle.project.image_name.clone());
        self.calibration = bundle.project.calibration;
        self.controls = bundle
            .project
            .controls
            .iter()
            .map(|c| c.track_index)
            .collect();
        self.view = bundle.project.view;
        self.fit_requested = false;
        self.recompute_metric_current();
        self.recompute_transform();
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
        self.bottom_graphs(ui);
        self.map_panel(ui);
        self.hover_km = self.pending_hover;
        self.hover_index = self.hover_km.and_then(|km| self.track_index_at_km(km));
        if self.hover_km.is_some() {
            ui.ctx().request_repaint(); // settle the 1-frame lag while hovering
        }
    }
}
