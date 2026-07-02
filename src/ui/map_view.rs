use crate::analysis::color_for;
use crate::app::{App, DragTarget, EditMode};
use crate::model::CalibrationPoint;
use egui::{Align2, Color32, FontId, Rect, Sense, Stroke, pos2};

const HIT_RADIUS: f32 = 12.0;
const SNAP_RADIUS: f32 = 40.0;

impl App {
    pub(crate) fn map_panel(&mut self, ui: &mut egui::Ui) {
        egui::CentralPanel::default().show(ui, |ui| {
            let size = ui.available_size();
            let (rect, resp) = ui.allocate_exact_size(size, Sense::click_and_drag());
            let origin = rect.min;

            let scroll = ui.input(|i| i.smooth_scroll_delta.y);
            // Ctrl/Cmd+Z removes the most recently added calibration point.
            let undo = ui.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::Z));
            if undo && self.calibration.pop().is_some() {
                self.recompute_transform();
                self.status = "Undid last calibration point.".into();
            }
            self.maybe_fit_view(rect);
            self.handle_zoom(&resp, origin, scroll);
            self.handle_interaction(&resp, origin);
            self.hover_route(&resp, origin);

            let painter = ui.painter_at(rect);
            painter.rect_filled(rect, 0.0, Color32::from_gray(30));
            self.draw_map(&painter, origin);
            self.draw_route(&painter, origin);
            self.draw_markers(&painter, origin);
        });
    }

    fn maybe_fit_view(&mut self, rect: Rect) {
        if !self.fit_requested {
            return;
        }
        if let Some(map) = &self.map {
            let (w, h) = (map.size[0] as f32, map.size[1] as f32);
            if rect.width() > 1.0 && w > 0.0 {
                let zoom = (rect.width() / w).min(rect.height() / h) * 0.95;
                self.view.zoom = zoom;
                self.view.offset = [
                    (rect.width() - w * zoom) / 2.0,
                    (rect.height() - h * zoom) / 2.0,
                ];
                self.fit_requested = false;
            }
        }
    }

    fn handle_zoom(&mut self, resp: &egui::Response, origin: egui::Pos2, scroll: f32) {
        if resp.hovered()
            && scroll != 0.0
            && let Some(p) = resp.hover_pos()
        {
            let img_before = self.to_image(origin, p);
            let factor = (scroll * 0.0015).exp();
            self.view.zoom = (self.view.zoom * factor).clamp(0.005, 200.0);
            let after = self.to_screen(origin, img_before);
            self.view.offset[0] += p.x - after.x;
            self.view.offset[1] += p.y - after.y;
        }
    }

    fn pan(&mut self, resp: &egui::Response) {
        let d = resp.drag_delta();
        self.view.offset[0] += d.x;
        self.view.offset[1] += d.y;
    }

    /// Hovering near the route reports the along-track position, so the graphs show
    /// a cursor there (cross-highlight with the graphs).
    fn hover_route(&mut self, resp: &egui::Response, origin: egui::Pos2) {
        if self.drag.is_none()
            && resp.hovered()
            && let Some(p) = resp.hover_pos()
            && let Some(idx) = self.nearest_waypoint(origin, p)
            && let Some(s) = self.waypoint_screen(origin, idx)
            && (s - p).length() < 30.0
        {
            self.pending_hover = self.km_at_index(idx);
        }
    }

    fn handle_interaction(&mut self, resp: &egui::Response, origin: egui::Pos2) {
        match self.mode {
            EditMode::Calibrate => self.handle_calibrate(resp, origin),
            EditMode::Control => {
                if resp.dragged() {
                    self.pan(resp);
                }
                if resp.clicked() {
                    self.handle_control_click(resp, origin);
                }
                if resp.secondary_clicked() {
                    self.remove_control_at(resp, origin);
                }
            }
        }
    }

    /// Right-click removes the control nearest the cursor.
    fn remove_control_at(&mut self, resp: &egui::Response, origin: egui::Pos2) {
        if let Some(p) = resp.interact_pointer_pos() {
            let mut best: Option<usize> = None;
            let mut best_d = SNAP_RADIUS;
            for (pos, &idx) in self.controls.iter().enumerate() {
                if let Some(s) = self.waypoint_screen(origin, idx) {
                    let d = (s - p).length();
                    if d < best_d {
                        best_d = d;
                        best = Some(pos);
                    }
                }
            }
            if let Some(pos) = best {
                self.controls.remove(pos);
                self.status = "Removed control.".into();
            }
        }
    }

    fn handle_calibrate(&mut self, resp: &egui::Response, origin: egui::Pos2) {
        // Right-click a pin to remove it.
        if resp.secondary_clicked()
            && let Some(p) = resp.interact_pointer_pos()
            && let Some(i) = self.pin_at(origin, p)
        {
            self.calibration.remove(i);
            self.recompute_transform();
            self.status = "Removed calibration point.".into();
            return;
        }
        // A press resolves to one of: grab an existing pin, create+lock a new pin on
        // the route, or (empty space) pan.
        if resp.drag_started()
            && let Some(p) = resp.interact_pointer_pos()
        {
            self.drag = Some(self.begin_calibrate_drag(origin, p));
        }
        if resp.dragged() {
            match self.drag {
                Some(DragTarget::Calibration(i)) => {
                    if let Some(p) = resp.interact_pointer_pos() {
                        let img = self.to_image(origin, p);
                        if let Some(c) = self.calibration.get_mut(i) {
                            c.image_px = [img.0, img.1];
                        }
                        self.recompute_transform();
                    }
                }
                _ => self.pan(resp),
            }
        }
        if resp.drag_stopped() {
            self.drag = None;
        }
    }

    /// Index of the calibration pin under a screen point, if any.
    fn pin_at(&self, origin: egui::Pos2, p: egui::Pos2) -> Option<usize> {
        self.calibration.iter().position(|c| {
            (self.to_screen(origin, (c.image_px[0], c.image_px[1])) - p).length() < HIT_RADIUS
        })
    }

    /// Decide what a calibrate-mode press starts dragging.
    fn begin_calibrate_drag(&mut self, origin: egui::Pos2, p: egui::Pos2) -> DragTarget {
        // 1. Re-grab an existing pin if the press landed on one.
        if let Some(i) = self.pin_at(origin, p) {
            return DragTarget::Calibration(i);
        }
        // 2. Otherwise, if the press is on the route, create a new pin locked to the
        //    nearest waypoint and drag it in the same gesture.
        if let Some(idx) = self.nearest_waypoint(origin, p)
            && let Some(s) = self.waypoint_screen(origin, idx)
            && (s - p).length() < SNAP_RADIUS
        {
            let img = self.to_image(origin, p);
            self.calibration.push(CalibrationPoint {
                track_index: idx,
                image_px: [img.0, img.1],
            });
            self.recompute_transform();
            self.status = "Drag onto the matching map feature, then release to lock.".into();
            return DragTarget::Calibration(self.calibration.len() - 1);
        }
        // 3. Empty space: pan.
        DragTarget::View
    }

    fn handle_control_click(&mut self, resp: &egui::Response, origin: egui::Pos2) {
        if let Some(p) = resp.interact_pointer_pos()
            && let Some(idx) = self.nearest_waypoint(origin, p)
            && let Some(s) = self.waypoint_screen(origin, idx)
            && (s - p).length() < SNAP_RADIUS
        {
            if let Some(pos) = self.controls.iter().position(|&x| x.abs_diff(idx) < 3) {
                self.controls.remove(pos);
            } else {
                self.controls.push(idx);
            }
            self.controls.sort_unstable();
            self.controls.dedup();
        }
    }

    fn draw_map(&self, painter: &egui::Painter, origin: egui::Pos2) {
        if let Some(map) = &self.map {
            let min = self.to_screen(origin, (0.0, 0.0));
            let max = self.to_screen(origin, (map.size[0] as f64, map.size[1] as f64));
            let uv = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
            painter.image(
                map.texture.id(),
                Rect::from_min_max(min, max),
                uv,
                Color32::WHITE,
            );
        }
    }

    fn draw_route(&self, painter: &egui::Painter, origin: egui::Pos2) {
        let Some(t) = &self.transform else { return };
        let n = self.projected.len();
        if n < 2 {
            return;
        }
        let width = (self.view.zoom * 6.0).clamp(1.5, 5.0);
        for i in 0..n - 1 {
            let p0 = self.to_screen(origin, t.apply(self.projected[i]));
            let p1 = self.to_screen(origin, t.apply(self.projected[i + 1]));
            let pace = self.seg_metric.get(i).copied().unwrap_or(f64::NAN);
            let color = color_for(pace, self.metric_range);
            painter.line_segment([p0, p1], Stroke::new(width, color));
        }
    }

    fn draw_markers(&self, painter: &egui::Painter, origin: egui::Pos2) {
        // Calibration pins: prominent locked markers (crosshair + ring) so it's clear
        // the route point is pinned to that exact map feature. Only shown while
        // calibrating; other modes hide them to keep the map clean.
        let cyan = Color32::from_rgb(0, 200, 255);
        for (i, c) in self
            .calibration
            .iter()
            .enumerate()
            .filter(|_| self.mode == EditMode::Calibrate)
        {
            let s = self.to_screen(origin, (c.image_px[0], c.image_px[1]));
            painter.circle_filled(s, 9.0, Color32::from_rgba_unmultiplied(0, 120, 160, 90));
            painter.circle_stroke(s, 9.0, Stroke::new(2.0, cyan));
            painter.line_segment(
                [s - egui::vec2(9.0, 0.0), s + egui::vec2(9.0, 0.0)],
                Stroke::new(1.0, cyan),
            );
            painter.line_segment(
                [s - egui::vec2(0.0, 9.0), s + egui::vec2(0.0, 9.0)],
                Stroke::new(1.0, cyan),
            );
            painter.text(
                s + egui::vec2(11.0, -11.0),
                Align2::LEFT_BOTTOM,
                format!("L{}", i + 1),
                FontId::proportional(11.0),
                cyan,
            );
        }
        // Cross-highlight marker from the graphs (or route hover).
        if let Some(i) = self.hover_index
            && let Some(s) = self.waypoint_screen(origin, i)
        {
            let yellow = Color32::from_rgb(255, 230, 0);
            painter.circle_stroke(s, 9.0, Stroke::new(2.5, yellow));
            painter.circle_filled(s, 3.0, yellow);
        }

        // Control markers, numbered.
        let Some(track) = &self.track else { return };
        let controls = crate::analysis::control_indices(track, &self.controls);
        for (n, &idx) in controls.iter().enumerate() {
            if let Some(s) = self.waypoint_screen(origin, idx) {
                painter.circle_filled(s, 8.0, Color32::from_rgb(230, 30, 120));
                painter.circle_stroke(s, 8.0, Stroke::new(1.5, Color32::WHITE));
                painter.text(
                    s,
                    Align2::CENTER_CENTER,
                    n.to_string(),
                    FontId::proportional(11.0),
                    Color32::WHITE,
                );
            }
        }
    }
}
