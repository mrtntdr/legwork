use crate::analysis::{compare, fmt_duration, fmt_pace, leg_label, quickness_color};
use crate::app::{App, EditMode, FitRequest, ViewTab};
use crate::athlete::route_color;
use egui::{Color32, RichText};

/// Highlight color for the best (fastest) athlete in the leg summary.
const BEST_GREEN: Color32 = Color32::from_rgb(80, 210, 120);

const IMAGE_EXTS: &[&str] = &["jpg", "jpeg", "png", "gif", "tiff", "tif", "bmp", "webp"];
const TRACK_EXTS: &[&str] = &["gpx", "tcx", "xml"];

/// Coloring palette handle colors and their minimum separation (min/km).
const QUICK_RED: egui::Color32 = egui::Color32::from_rgb(240, 70, 70);
const SLOW_BLUE: egui::Color32 = egui::Color32::from_rgb(90, 130, 255);
const MIN_GAP: f64 = 0.1;
/// Fraction of the palette bar left as a solid-color margin on each side, so the
/// gradient fills the middle ~70% and the knobs rest ~15% in from each edge.
const PALETTE_MARGIN: f64 = 0.15;
/// Minimum cutoff span (min/km) used when reframing, so a degenerate quick≈slow
/// still yields a usable bar.
const MIN_SPAN: f64 = 0.5;

/// The palette bar range `(lo, hi)` in min/km that frames the given cutoffs so the
/// gradient fills the middle of the bar and the knobs rest `PALETTE_MARGIN` in from
/// each edge. `hi` is the slow (left) end, `lo` the quick (right) end.
fn reframe_palette(quick: f64, slow: f64) -> (f64, f64) {
    let span = (slow - quick).max(MIN_SPAN);
    let full = span / (1.0 - 2.0 * PALETTE_MARGIN);
    let hi = slow + PALETTE_MARGIN * full;
    let lo = (quick - PALETTE_MARGIN * full).max(0.0);
    (lo, hi)
}

impl App {
    pub(crate) fn top_bar(&mut self, ui: &mut egui::Ui) {
        egui::Panel::top("top_bar").show(ui, |ui| {
            // A single horizontal row keeps the bar a fixed height (nested fill layouts
            // like columns/vertical_centered/justified make a top panel grow on hover).
            // Groups: File menu · the two activity tabs · Setup's edit modes.
            ui.horizontal(|ui| {
                self.file_menu(ui);
                ui.separator();
                ui.selectable_value(&mut self.tab, ViewTab::Setup, "Setup");
                ui.selectable_value(&mut self.tab, ViewTab::Analysis, "Analysis");
                if self.tab == ViewTab::Setup {
                    ui.separator();
                    ui.selectable_value(&mut self.mode, EditMode::Calibrate, "Calibrate");
                    ui.selectable_value(&mut self.mode, EditMode::Control, "Course");
                }
            });
        });

        egui::Panel::bottom("status_bar").show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(&self.status);
            });
        });
    }

    /// All file operations in one place, so the top bar stays about the two
    /// activities rather than a row of dialogs.
    fn file_menu(&mut self, ui: &mut egui::Ui) {
        ui.menu_button("File", |ui| {
            if ui.button("Open Map…").clicked()
                && let Some(path) = rfd::FileDialog::new()
                    .add_filter("Images", IMAGE_EXTS)
                    .pick_file()
                && let Ok(bytes) = std::fs::read(&path)
            {
                let name = file_name(&path);
                // World-file sidecar or embedded GeoTIFF tags, when present, let
                // tracks and IOF courses land on the map with no calibration.
                let georef = crate::io::detect_georef(&path, &bytes);
                let ctx = ui.ctx().clone();
                self.load_image_from_bytes(&ctx, bytes, name, georef);
            }
            if ui.button("Add Track…").clicked() {
                self.add_athlete_dialog();
            }
            if ui
                .button("Import Course…")
                .on_hover_text("IOF XML 3.0 course file (OCAD, Purple Pen, Condes)")
                .clicked()
                && let Some(path) = rfd::FileDialog::new()
                    .add_filter("IOF XML course", &["xml"])
                    .pick_file()
                && let Ok(bytes) = std::fs::read(&path)
            {
                self.import_course(&bytes);
            }
            ui.separator();
            if ui.button("Open Project…").clicked()
                && let Some(path) = rfd::FileDialog::new()
                    .add_filter("Legwork project", &["legit", "route"])
                    .pick_file()
            {
                let ctx = ui.ctx().clone();
                self.open_project(&ctx, path);
            }
            if ui.button("Save Project…").clicked()
                && let Some(path) = rfd::FileDialog::new()
                    .add_filter("Legwork project", &["legit"])
                    .set_file_name("analysis.legit")
                    .save_file()
            {
                self.save_project(path);
            }
            ui.separator();
            if ui.button("Export PNG…").clicked() {
                self.export_png();
            }
        });
    }

    /// Pick a GPX/TCX file and add it as a new athlete.
    fn add_athlete_dialog(&mut self) {
        if let Some(path) = rfd::FileDialog::new()
            .add_filter("GPX/TCX", TRACK_EXTS)
            .pick_file()
            && let Ok(bytes) = std::fs::read(&path)
        {
            let name = file_name(&path);
            self.add_athlete(bytes, name);
        }
    }

    pub(crate) fn side_panel(&mut self, ui: &mut egui::Ui) {
        egui::Panel::right("side")
            .default_size(360.0)
            .show(ui, |ui| match self.tab {
                ViewTab::Setup => self.setup_side_panel(ui),
                ViewTab::Analysis => self.analysis_side_panel(ui),
            });
    }

    /// Setup: full athlete management, the active track's stats, and the details
    /// of whichever edit mode is active (calibration or course).
    fn setup_side_panel(&mut self, ui: &mut egui::Ui) {
        self.athletes_section(ui, true);

        ui.separator();
        ui.heading("Track");
        if let Some(a) = self.active() {
            ui.label(format!("Points: {}", a.track.len()));
            ui.label(format!(
                "Distance: {:.2} km",
                a.track.total_distance() / 1000.0
            ));
            if let Some(d) = a.track.duration_secs() {
                ui.label(format!("Duration: {}", fmt_duration(d)));
            }
        } else {
            ui.label("No track loaded.");
        }

        self.map_section(ui);

        match self.mode {
            EditMode::Calibrate => self.calibration_section(ui),
            EditMode::Control => self.course_section(ui),
        }
    }

    /// Map orientation controls (Setup): rotate the whole view in 90° steps or by a
    /// fine angle, so a sideways photo/scan can be turned upright. Routes, controls
    /// and pins rotate with the map, and the angle is saved with the project.
    fn map_section(&mut self, ui: &mut egui::Ui) {
        use std::f32::consts::{FRAC_PI_2, PI};
        if self.map.is_none() {
            return;
        }
        ui.separator();
        ui.heading("Map");
        ui.horizontal(|ui| {
            ui.label("Rotate");
            if ui.button("⟲").on_hover_text("Rotate 90° left").clicked() {
                self.rotate_by(-FRAC_PI_2);
            }
            if ui.button("⟳").on_hover_text("Rotate 90° right").clicked() {
                self.rotate_by(FRAC_PI_2);
            }
            if ui
                .add_enabled(self.view.rotation != 0.0, egui::Button::new("Reset"))
                .on_hover_text("Back to image-up")
                .clicked()
            {
                self.rotate_to(0.0);
            }
        });
        let mut deg = self.view.rotation.to_degrees();
        let range = (-PI).to_degrees()..=PI.to_degrees();
        if ui
            .add(egui::Slider::new(&mut deg, range).suffix("°").text("angle"))
            .on_hover_text("Fine rotation to straighten an angled photo")
            .changed()
        {
            self.rotate_to(deg.to_radians());
        }
    }

    /// Analysis: a compact athlete list (visibility + active pick), route coloring
    /// (only while pace colors are in use), and either the overall leaderboard
    /// (whole course) or the selected-leg summary. Nothing that edits the project.
    fn analysis_side_panel(&mut self, ui: &mut egui::Ui) {
        self.athletes_section(ui, false);
        if self.active_pace_colors {
            self.coloring_controls(ui);
        }
        self.routes_section(ui);
        match self.selected_leg {
            Some(_) => self.leg_summary_section(ui),
            None => self.leaderboard_section(ui),
        }
    }

    /// Analysis: the drawn route options (analysis board). With a leg selected it
    /// lists that leg's variants — length, delta to the shortest, points — plus the
    /// athletes' actual distances on the leg. In the whole-course view it lists
    /// every route.
    fn routes_section(&mut self, ui: &mut egui::Ui) {
        ui.separator();
        ui.horizontal(|ui| {
            ui.heading("Route options");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .selectable_label(self.draw_mode, "✏ Draw")
                    .on_hover_text("Draw route options on the map (D)")
                    .clicked()
                {
                    self.toggle_draw_mode();
                }
            });
        });
        ui.label(
            RichText::new(
                "Click to drop points, drag to sketch. Double-click or Enter to finish, Esc to cancel.",
            )
            .weak()
            .small(),
        );

        let leg = self.selected_leg;
        let scored = self.controls.iter().any(|c| c.score.is_some());
        let show: Vec<usize> = (0..self.drawn_routes.len())
            .filter(|&i| match leg {
                None => true,
                Some(li) => self.drawn_routes[i].leg == Some(li),
            })
            .collect();

        if show.is_empty() {
            ui.label(RichText::new("No route options yet — turn on Draw.").weak().small());
        } else {
            let shortest = show
                .iter()
                .filter_map(|&i| self.drawn_stats.get(i).and_then(|s| s.length_m))
                .fold(f64::INFINITY, f64::min);
            let mut select: Option<usize> = None;
            let mut delete: Option<usize> = None;
            for (n, &i) in show.iter().enumerate() {
                ui.horizontal(|ui| {
                    let mut col = route_color(&self.drawn_routes[i], i);
                    if ui.color_edit_button_srgba(&mut col).changed() {
                        self.drawn_routes[i].color = Some([col.r(), col.g(), col.b()]);
                    }
                    let name = route_display_name(&self.drawn_routes[i], n, self.controls.len());
                    if ui
                        .selectable_label(self.selected_route == Some(i), name)
                        .clicked()
                    {
                        select = Some(i);
                    }
                    let len = self.drawn_stats.get(i).and_then(|s| s.length_m);
                    ui.label(
                        len.map(|m| format!("{m:.0} m"))
                            .unwrap_or_else(|| "— m".into()),
                    );
                    if let Some(m) = len
                        && shortest.is_finite()
                        && m > shortest + 0.5
                    {
                        ui.label(RichText::new(format!("+{:.0}", m - shortest)).weak().small());
                    }
                    if scored
                        && let Some(s) = self.drawn_stats.get(i)
                        && s.points > 0
                    {
                        ui.label(RichText::new(format!("· {} p", s.points)).weak().small());
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("✕").on_hover_text("Delete route").clicked() {
                            delete = Some(i);
                        }
                    });
                });
            }
            if let Some(i) = select {
                self.selected_route = Some(i);
                if let Some((min, max)) = self.route_bbox(i) {
                    self.fit = Some(FitRequest::Rect { min, max });
                }
            }
            if let Some(i) = delete {
                self.drawn_routes.remove(i);
                if self.selected_route == Some(i) {
                    self.selected_route = None;
                }
                self.recompute_drawn_stats();
            }
        }

        // The athletes' actual distances on the selected leg, for comparison.
        if let Some(li) = leg {
            let rows: Vec<(Color32, String, Option<f64>)> = self
                .athletes
                .iter()
                .filter(|a| a.visible)
                .map(|a| {
                    let b = a.boundaries();
                    let m = match (
                        b.get(li).copied().flatten(),
                        b.get(li + 1).copied().flatten(),
                    ) {
                        (Some(f), Some(t)) if f <= t => Some(a.track.route_length(f, t)),
                        _ => None,
                    };
                    (a.color, a.name.clone(), m)
                })
                .collect();
            if !rows.is_empty() {
                ui.add_space(2.0);
                ui.label(RichText::new("Ran on this leg:").weak().small());
                for (color, name, m) in rows {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("●").color(color));
                        ui.label(&name);
                        match m {
                            Some(m) => ui.label(RichText::new(format!("{m:.0} m")).weak()),
                            None => ui.label(RichText::new("–").weak()),
                        };
                    });
                }
            }
        }
    }

    /// Overall standings while the whole course is shown: visible athletes ranked
    /// by total time, with the gap to the leader.
    fn leaderboard_section(&self, ui: &mut egui::Ui) {
        let mut rows: Vec<(usize, Option<f64>)> = (0..self.athletes.len())
            .filter(|&i| self.athletes[i].visible)
            .map(|i| (i, self.athletes[i].track.duration_secs()))
            .collect();
        if rows.is_empty() {
            return;
        }
        // Timed athletes first, fastest to slowest; untimed tracks at the end.
        rows.sort_by(|a, b| match (a.1, b.1) {
            (Some(x), Some(y)) => x.total_cmp(&y),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => std::cmp::Ordering::Equal,
        });
        let best = rows.first().and_then(|&(_, t)| t);

        ui.separator();
        ui.heading("Leaderboard");
        for (place, &(i, total)) in rows.iter().enumerate() {
            let a = &self.athletes[i];
            ui.horizontal(|ui| {
                ui.label(format!("{}.", place + 1));
                ui.label(RichText::new("●").color(a.color));
                ui.label(&a.name);
                match total {
                    Some(secs) => {
                        let is_best = best.is_some_and(|b| (secs - b).abs() < 0.5);
                        let text = RichText::new(fmt_duration(secs)).strong();
                        ui.label(if is_best { text.color(BEST_GREEN) } else { text });
                        if let Some(b) = best
                            && !is_best
                        {
                            ui.label(
                                RichText::new(format!("+{}", fmt_duration(secs - b)))
                                    .weak()
                                    .small(),
                            );
                        }
                    }
                    None => {
                        ui.label(RichText::new("no time").weak());
                    }
                }
            });
        }
    }

    /// Course editing details (Setup · Course mode).
    fn course_section(&mut self, ui: &mut egui::Ui) {
        ui.separator();
        ui.heading("Course");
        ui.label(format!("Controls: {}", self.controls.len()));
        if !self.controls.is_empty() && ui.button("Clear controls").clicked() {
            self.controls.clear();
            self.rematch_all();
        }
        ui.label(
            egui::RichText::new(
                "Click the map to place a control, drag to move it, right-click to remove.",
            )
            .weak()
            .small(),
        );

        if !self.controls.is_empty() {
            let mut changed = false;
            egui::CollapsingHeader::new("Scores (rogaine)")
                .default_open(false)
                .show(ui, |ui| {
                    ui.label(
                        egui::RichText::new("Point value per control; 0 = no score.")
                            .weak()
                            .small(),
                    );
                    for (i, c) in self.controls.iter_mut().enumerate() {
                        ui.horizontal(|ui| {
                            ui.label(format!("{}", i + 1));
                            let mut v = c.score.unwrap_or(0) as i32;
                            if ui
                                .add(egui::DragValue::new(&mut v).range(0..=1000).speed(1.0))
                                .changed()
                            {
                                c.score = (v > 0).then_some(v as u32);
                                changed = true;
                            }
                        });
                    }
                    if ui.button("Clear scores").clicked() {
                        for c in self.controls.iter_mut() {
                            c.score = None;
                        }
                        changed = true;
                    }
                });
            if changed {
                self.recompute_drawn_stats();
            }
        }
    }

    /// When a leg is selected on the map, a compact per-athlete summary for that
    /// leg (time + delta to best, pace, length) with a way back to the full course.
    fn leg_summary_section(&mut self, ui: &mut egui::Ui) {
        let Some(li) = self.selected_leg else { return };
        let visible: Vec<usize> = (0..self.athletes.len())
            .filter(|&i| self.athletes[i].visible)
            .collect();

        ui.separator();
        let mut show_all = false;
        ui.horizontal(|ui| {
            ui.heading(format!("Leg {}", leg_label(li, self.controls.len())));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Show all legs").clicked() {
                    show_all = true;
                }
            });
        });

        if visible.is_empty() {
            ui.label(RichText::new("No visible athletes.").weak());
        } else {
            let boundaries: Vec<Vec<Option<usize>>> = visible
                .iter()
                .map(|&i| self.athletes[i].boundaries())
                .collect();
            let entries: Vec<_> = visible
                .iter()
                .zip(&boundaries)
                .map(|(&i, b)| (&self.athletes[i].track, b.as_slice()))
                .collect();
            let rows = compare(&entries, self.controls.len());
            if let Some(row) = rows.get(li) {
                let best_secs = row
                    .best
                    .and_then(|b| row.cells[b].leg.as_ref())
                    .and_then(|l| l.duration_secs);
                for (ci, &ai) in visible.iter().enumerate() {
                    let a = &self.athletes[ai];
                    let cell = &row.cells[ci];
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("●").color(a.color));
                        ui.label(&a.name);
                        match cell.leg.as_ref() {
                            Some(leg) => {
                                match leg.duration_secs {
                                    Some(secs) => {
                                        let is_best = row.best == Some(ci);
                                        let text = RichText::new(fmt_duration(secs)).strong();
                                        ui.label(if is_best {
                                            text.color(BEST_GREEN)
                                        } else {
                                            text
                                        });
                                        if let Some(b) = best_secs
                                            && !is_best
                                        {
                                            ui.label(
                                                RichText::new(format!(
                                                    "+{}",
                                                    fmt_duration(secs - b)
                                                ))
                                                .weak()
                                                .small(),
                                            );
                                        }
                                    }
                                    None => {
                                        ui.label(RichText::new("no time").weak());
                                    }
                                }
                                let pace = leg
                                    .pace_s_per_km
                                    .map(fmt_pace)
                                    .unwrap_or_else(|| "–".into());
                                ui.label(
                                    RichText::new(format!("· {pace} · {:.0} m", leg.route_length))
                                        .weak()
                                        .small(),
                                );
                            }
                            None => {
                                ui.label(RichText::new("– (control missed)").weak());
                            }
                        }
                    });
                }
            }
        }
        if show_all {
            self.select_leg(None);
        }
    }

    /// The athlete list. `full` (Setup) shows management — editable name, color
    /// picker, remove, add. Compact (Analysis) is just color · visibility · pick
    /// the active athlete.
    fn athletes_section(&mut self, ui: &mut egui::Ui, full: bool) {
        ui.heading("Athletes");
        let mut remove: Option<usize> = None;
        let mut make_active: Option<usize> = None;
        let active = self.active;
        for (i, a) in self.athletes.iter_mut().enumerate() {
            ui.horizontal(|ui| {
                ui.color_edit_button_srgba(&mut a.color);
                ui.checkbox(&mut a.visible, "").on_hover_text("Show route");
                if full && i == active {
                    ui.add(
                        egui::TextEdit::singleline(&mut a.name)
                            .desired_width(ui.available_width() - 30.0),
                    );
                } else if ui
                    .selectable_label(i == active, &a.name)
                    .on_hover_text("Click to make active")
                    .clicked()
                {
                    make_active = Some(i);
                }
                if full {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("✕").on_hover_text("Remove athlete").clicked() {
                            remove = Some(i);
                        }
                    });
                }
            });
        }
        if let Some(i) = make_active {
            self.set_active(i);
        }
        if let Some(i) = remove {
            self.remove_athlete(i);
            self.status = "Removed athlete.".into();
        }
        if full {
            if ui.button("Add Athlete…").clicked() {
                self.add_athlete_dialog();
            }
            if self.athletes.is_empty() {
                ui.label(
                    egui::RichText::new("Add a GPS track to begin.")
                        .weak()
                        .small(),
                );
            }
        } else if !self.athletes.is_empty() {
            ui.checkbox(&mut self.active_pace_colors, "Pace-color active route");
            ui.label(
                egui::RichText::new("The highlighted athlete drives graphs and pace colors.")
                    .weak()
                    .small(),
            );
        }
    }

    fn calibration_section(&mut self, ui: &mut egui::Ui) {
        ui.separator();
        ui.heading("Calibration");
        let Some(a) = self.active() else {
            ui.label("Add a track to calibrate.");
            return;
        };
        ui.label(format!("Points: {}", a.calibration.len()));
        if let (Some(t), true) = (&a.transform, a.calibration.len() >= 2) {
            let pts: Vec<_> = a
                .calibration
                .iter()
                .filter_map(|c| {
                    a.projected
                        .get(c.track_index)
                        .map(|&m| (m, (c.image_px[0], c.image_px[1])))
                })
                .collect();
            ui.label(format!("Fit residual: {:.1} px", t.rms_residual(&pts)));
        } else {
            ui.label("Add ≥2 points (Calibrate mode) to fit.");
        }
        // Per-point removal (also: right-click a marker, or Ctrl/Cmd+Z to undo).
        let n_points = a.calibration.len();
        let mut remove: Option<usize> = None;
        for i in 0..n_points {
            ui.horizontal(|ui| {
                ui.label(format!("L{}", i + 1));
                if ui.button("Remove").clicked() {
                    remove = Some(i);
                }
            });
        }
        if let Some(i) = remove {
            if let Some(a) = self.active_mut() {
                a.calibration.remove(i);
            }
            self.recompute_transform_active();
            self.status = "Removed calibration point.".into();
        }
        if n_points > 0 && ui.button("Clear calibration").clicked() {
            if let Some(a) = self.active_mut() {
                a.calibration.clear();
            }
            self.recompute_transform_active();
        }
        ui.label(
            egui::RichText::new("Right-click a marker or Ctrl/Cmd+Z to remove.")
                .weak()
                .small(),
        );
    }

    /// Route coloring controls: an interactive pace/color palette whose two handles
    /// (red = quick, blue = slow) set the cutoffs, plus an auto toggle.
    fn coloring_controls(&mut self, ui: &mut egui::Ui) {
        ui.separator();
        ui.heading("Coloring (pace)");

        let mut auto = self.color_auto;
        if ui
            .checkbox(&mut auto, "Auto range")
            .on_hover_text("Fit the color range to this run. Dragging a handle switches to manual.")
            .changed()
        {
            self.color_auto = auto;
            if auto {
                self.recompute_metric_active();
            }
        }

        // The bar frames the current cutoffs (so the gradient fills its middle), except
        // while a knob is being dragged, when the frame is frozen for live feedback and
        // only reframes on release.
        let (mut quick, mut slow) = self.friendly_cutoffs();
        let (lo, hi) = self
            .palette_view
            .unwrap_or_else(|| reframe_palette(quick, slow));

        // Palette widget: left = slow (high pace, blue), right = quick (low pace, red).
        let bar_h = 20.0;
        let (rect, _) = ui.allocate_exact_size(
            egui::vec2(ui.available_width(), bar_h + 32.0),
            egui::Sense::hover(),
        );
        let bar = egui::Rect::from_min_max(rect.min, egui::pos2(rect.right(), rect.top() + bar_h));
        let x_of = |p: f64| bar.left() + ((hi - p) / (hi - lo)) as f32 * bar.width();
        let p_of =
            |x: f32| hi - ((x - bar.left()) / bar.width()).clamp(0.0, 1.0) as f64 * (hi - lo);

        // Draggable handles (hit-tested at their current positions).
        let base = ui.id();
        let hit = |c: f32| {
            egui::Rect::from_center_size(
                egui::pos2(c, rect.center().y),
                egui::vec2(22.0, rect.height()),
            )
        };
        let slow_resp = ui.interact(
            hit(x_of(slow)),
            base.with("lever_slow"),
            egui::Sense::drag(),
        );
        let quick_resp = ui.interact(
            hit(x_of(quick)),
            base.with("lever_quick"),
            egui::Sense::drag(),
        );
        // Freeze the current (rest) frame when a drag begins so the knob moves within a
        // stable bar; clear it on release so the next frame reframes to the new cutoffs.
        if slow_resp.drag_started() || quick_resp.drag_started() {
            self.palette_view = Some((lo, hi));
        }
        let mut changed = false;
        if slow_resp.dragged()
            && let Some(p) = slow_resp.interact_pointer_pos()
        {
            slow = p_of(p.x).clamp(quick + MIN_GAP, hi);
            changed = true;
        }
        if quick_resp.dragged()
            && let Some(p) = quick_resp.interact_pointer_pos()
        {
            quick = p_of(p.x).clamp(lo, slow - MIN_GAP);
            changed = true;
        }
        if slow_resp.drag_stopped() || quick_resp.drag_stopped() {
            self.palette_view = None;
        }

        // Gradient reflecting the (possibly just-updated) cutoffs.
        let range = crate::analysis::MetricRange {
            min: quick * 60.0,
            max: slow * 60.0,
        };
        let painter = ui.painter_at(rect);
        let steps = 64;
        for k in 0..steps {
            let x0 = bar.left() + bar.width() * k as f32 / steps as f32;
            let x1 = bar.left() + bar.width() * (k + 1) as f32 / steps as f32;
            let q = 1.0 - range.normalize(p_of((x0 + x1) * 0.5) * 60.0);
            painter.rect_filled(
                egui::Rect::from_min_max(egui::pos2(x0, bar.top()), egui::pos2(x1, bar.bottom())),
                0.0,
                quickness_color(q, 255),
            );
        }

        // Handles: a line through the bar and a grab knob below.
        let handle = |x: f32, color: egui::Color32| {
            painter.line_segment(
                [egui::pos2(x, bar.top()), egui::pos2(x, bar.bottom())],
                egui::Stroke::new(2.0, egui::Color32::WHITE),
            );
            painter.circle(
                egui::pos2(x, bar.bottom() + 8.0),
                6.0,
                color,
                egui::Stroke::new(1.5, egui::Color32::WHITE),
            );
        };
        handle(x_of(slow), SLOW_BLUE);
        handle(x_of(quick), QUICK_RED);

        // Pace scale: evenly spaced absolute min/km ticks (left = slow, right = quick).
        let font = egui::FontId::proportional(10.0);
        let ticks = 5;
        for k in 0..ticks {
            let fx = k as f32 / (ticks - 1) as f32;
            let x = bar.left() + fx * bar.width();
            let pace = hi - fx as f64 * (hi - lo);
            painter.line_segment(
                [
                    egui::pos2(x, bar.bottom()),
                    egui::pos2(x, bar.bottom() + 3.0),
                ],
                egui::Stroke::new(1.0, egui::Color32::from_gray(80)),
            );
            let align = if k == 0 {
                egui::Align2::LEFT_TOP
            } else if k == ticks - 1 {
                egui::Align2::RIGHT_TOP
            } else {
                egui::Align2::CENTER_TOP
            };
            painter.text(
                egui::pos2(x, bar.bottom() + 18.0),
                align,
                fmt_duration(pace * 60.0),
                font.clone(),
                egui::Color32::GRAY,
            );
        }

        if changed {
            self.set_friendly_cutoffs(quick, slow);
        }
        ui.label(
            egui::RichText::new(format!(
                "slow (blue) {} · quick (red) {}",
                fmt_pace(slow * 60.0),
                fmt_pace(quick * 60.0)
            ))
            .small()
            .weak(),
        );
    }
}

fn file_name(path: &std::path::Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "file".into())
}

/// The label shown for a drawn route: its user name, else an auto-name — a variant
/// letter per leg ("2–3 A"), or "Route n" for a free-form route. `n` is the
/// route's position within the list currently shown.
fn route_display_name(r: &crate::model::DrawnRoute, n: usize, n_controls: usize) -> String {
    if !r.name.is_empty() {
        return r.name.clone();
    }
    let letter = (b'A' + (n % 26) as u8) as char;
    match r.leg {
        Some(li) => format!("{} {letter}", leg_label(li, n_controls)),
        None => format!("Route {}", n + 1),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reframe_palette_frames_cutoffs_with_margins() {
        let (quick, slow) = (4.0, 6.0);
        let (lo, hi) = reframe_palette(quick, slow);
        assert!(
            lo < quick && hi > slow,
            "cutoffs must sit inside ({lo}, {hi})"
        );
        // The cutoff span fills the middle 1 - 2*margin of the bar.
        let filled = (slow - quick) / (hi - lo);
        assert!(
            (filled - (1.0 - 2.0 * PALETTE_MARGIN)).abs() < 1e-9,
            "filled fraction {filled}"
        );
    }

    #[test]
    fn reframe_palette_handles_degenerate_and_near_zero_cutoffs() {
        // Equal cutoffs still produce a non-empty bar containing them.
        let (lo, hi) = reframe_palette(5.0, 5.0);
        assert!(hi > lo);
        assert!(lo < 5.0 && hi > 5.0);
        // The low end never goes below zero pace.
        let (lo, _) = reframe_palette(0.05, 0.2);
        assert!(lo >= 0.0);
    }

    #[test]
    fn file_name_falls_back_when_pathless() {
        assert_eq!(file_name(std::path::Path::new("/a/b/run.gpx")), "run.gpx");
        assert_eq!(file_name(std::path::Path::new("/")), "file");
    }
}
