use crate::analysis::{fmt_duration, fmt_pace, legs, quickness_color};
use crate::app::{App, EditMode};
use egui_extras::{Column, TableBuilder};

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
            // Groups: Open (left) · Modes · Save/Export (right-aligned).
            ui.horizontal(|ui| {
                self.open_group(ui);
                ui.separator();
                ui.selectable_value(&mut self.mode, EditMode::Calibrate, "Calibrate");
                ui.selectable_value(&mut self.mode, EditMode::Control, "Controls");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    self.save_group(ui);
                });
            });
        });

        egui::Panel::bottom("status_bar").show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(&self.status);
            });
        });
    }

    fn open_group(&mut self, ui: &mut egui::Ui) {
        if ui.button("Open Map…").clicked()
            && let Some(path) = rfd::FileDialog::new()
                .add_filter("Images", IMAGE_EXTS)
                .pick_file()
            && let Ok(bytes) = std::fs::read(&path)
        {
            let name = file_name(&path);
            let ctx = ui.ctx().clone();
            self.load_image_from_bytes(&ctx, bytes, name);
        }
        if ui.button("Open Track…").clicked()
            && let Some(path) = rfd::FileDialog::new()
                .add_filter("GPX/TCX", TRACK_EXTS)
                .pick_file()
            && let Ok(bytes) = std::fs::read(&path)
        {
            let name = file_name(&path);
            self.load_track_from_bytes(bytes, name);
        }
    }

    /// Save/export group, laid out right-to-left, so it reads Save · Open · Export.
    fn save_group(&mut self, ui: &mut egui::Ui) {
        if ui.button("Export PNG…").clicked() {
            self.export_png();
        }
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
    }

    pub(crate) fn side_panel(&mut self, ui: &mut egui::Ui) {
        egui::Panel::right("side")
            .default_size(360.0)
            .show(ui, |ui| {
                ui.heading("Track");
                if let Some(track) = &self.track {
                    ui.label(format!("Points: {}", track.len()));
                    ui.label(format!(
                        "Distance: {:.2} km",
                        track.total_distance() / 1000.0
                    ));
                    if let Some(d) = track.duration_secs() {
                        ui.label(format!("Duration: {}", fmt_duration(d)));
                    }
                } else {
                    ui.label("No track loaded.");
                }

                // The calibration section is only relevant while calibrating.
                if self.mode == EditMode::Calibrate {
                    ui.separator();
                    ui.heading("Calibration");
                    ui.label(format!("Points: {}", self.calibration.len()));
                    if let (Some(t), true) = (&self.transform, self.calibration.len() >= 2) {
                        let pts: Vec<_> = self
                            .calibration
                            .iter()
                            .filter_map(|c| {
                                self.projected
                                    .get(c.track_index)
                                    .map(|&m| (m, (c.image_px[0], c.image_px[1])))
                            })
                            .collect();
                        ui.label(format!("Fit residual: {:.1} px", t.rms_residual(&pts)));
                    } else {
                        ui.label("Add ≥2 points (Calibrate mode) to fit.");
                    }
                    // Per-point removal (also: right-click a marker, or Ctrl/Cmd+Z to undo).
                    let mut remove: Option<usize> = None;
                    for i in 0..self.calibration.len() {
                        ui.horizontal(|ui| {
                            ui.label(format!("L{}", i + 1));
                            if ui.button("Remove").clicked() {
                                remove = Some(i);
                            }
                        });
                    }
                    if let Some(i) = remove {
                        self.calibration.remove(i);
                        self.recompute_transform();
                        self.status = "Removed calibration point.".into();
                    }
                    if !self.calibration.is_empty() && ui.button("Clear calibration").clicked() {
                        self.calibration.clear();
                        self.recompute_transform();
                    }
                    ui.label(
                        egui::RichText::new("Right-click a marker or Ctrl/Cmd+Z to remove.")
                            .weak()
                            .small(),
                    );
                }

                // Coloring and graph toggles belong to leg analysis (Controls mode).
                if self.mode == EditMode::Control {
                    self.coloring_controls(ui);

                    ui.separator();
                    ui.heading("Graphs");
                    ui.checkbox(&mut self.show_pace, "Pace");
                    ui.checkbox(&mut self.show_hr, "Heart rate");
                    ui.checkbox(&mut self.show_ele, "Elevation");
                }

                ui.separator();
                ui.heading("Controls");
                if !self.controls.is_empty() && ui.button("Clear controls").clicked() {
                    self.controls.clear();
                }
                self.legs_table(ui);
            });
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
                self.recompute_metric_current();
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

    fn legs_table(&mut self, ui: &mut egui::Ui) {
        let Some(track) = &self.track else {
            ui.label("Load a track to see legs.");
            return;
        };
        let legs = legs(track, &self.controls);
        if legs.is_empty() {
            ui.label("No legs yet.");
            return;
        }
        TableBuilder::new(ui)
            .striped(true)
            .column(Column::exact(36.0))
            .column(Column::exact(60.0))
            .column(Column::exact(70.0))
            .column(Column::exact(60.0))
            .column(Column::remainder())
            .header(18.0, |mut h| {
                for label in ["Leg", "Time", "Length", "Detour", "Pace"] {
                    h.col(|ui| {
                        ui.strong(label);
                    });
                }
            })
            .body(|mut body| {
                for (i, leg) in legs.iter().enumerate() {
                    body.row(18.0, |mut row| {
                        row.col(|ui| {
                            ui.label(format!("{}", i + 1));
                        });
                        row.col(|ui| {
                            ui.label(
                                leg.duration_secs
                                    .map(fmt_duration)
                                    .unwrap_or_else(|| "–".into()),
                            );
                        });
                        row.col(|ui| {
                            ui.label(format!("{:.0} m", leg.route_length));
                        });
                        row.col(|ui| {
                            ui.label(format!("{:+.0}%", leg.detour_pct));
                        });
                        row.col(|ui| {
                            ui.label(
                                leg.pace_s_per_km
                                    .map(fmt_pace)
                                    .unwrap_or_else(|| "–".into()),
                            );
                        });
                    });
                }
            });
    }
}

fn file_name(path: &std::path::Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "file".into())
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
