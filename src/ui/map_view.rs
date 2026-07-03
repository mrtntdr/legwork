use crate::analysis::color_for;
use crate::app::{App, DragTarget, EditMode};
use crate::athlete::Athlete;
use crate::model::{CalibrationPoint, CoursePoint};
use egui::{Align2, Color32, FontId, Rect, Sense, Stroke, pos2};

const HIT_RADIUS: f32 = 12.0;
const SNAP_RADIUS: f32 = 40.0;

impl App {
    pub(crate) fn map_panel(&mut self, ui: &mut egui::Ui) {
        egui::CentralPanel::default().show(ui, |ui| {
            let size = ui.available_size();
            let (rect, resp) = ui.allocate_exact_size(size, Sense::click_and_drag());
            let origin = rect.min;

            // Ctrl/Cmd+Z removes the active athlete's most recent calibration point.
            let undo = ui.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::Z));
            if undo && self.active_mut().is_some_and(|a| a.calibration.pop().is_some()) {
                self.recompute_transform_active();
                self.status = "Undid last calibration point.".into();
            }
            self.maybe_fit_view(rect);
            self.handle_zoom_pan(ui, &resp, origin);
            self.handle_interaction(&resp, origin);
            self.hover_route(&resp, origin);

            let painter = ui.painter_at(rect);
            painter.rect_filled(rect, 0.0, Color32::from_gray(30));
            self.draw_map(&painter, origin);
            self.draw_routes(&painter, origin);
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

    /// Zoom and pan from wheel/touchpad input, gated on the cursor being over the map.
    ///
    /// Three gestures feed in, kept distinct so mouse and touchpad both feel right:
    /// - **Pinch** (touchpad) and **Ctrl+scroll** arrive as a zoom factor via
    ///   `zoom_delta()`. On Windows precision touchpads the OS delivers pinch as
    ///   Ctrl+scroll, so this covers touchpad pinch there too.
    /// - **Two-finger swipe** on a precision touchpad arrives as pixel-unit
    ///   (`MouseWheelUnit::Point`) scrolling, which pans the view.
    /// - A plain **mouse wheel** arrives as line-unit scrolling, which zooms
    ///   (preserving the classic scroll-to-zoom feel).
    ///
    /// Zoom is always anchored on the pointer so the map feature under the cursor
    /// stays put.
    fn handle_zoom_pan(&mut self, ui: &egui::Ui, resp: &egui::Response, origin: egui::Pos2) {
        if !resp.hovered() {
            return;
        }
        let Some(p) = resp.hover_pos() else { return };

        let mut pan = egui::Vec2::ZERO;
        let mut wheel_zoom = 0.0_f32;
        let pinch = ui.input(|i| {
            for ev in &i.events {
                if let egui::Event::MouseWheel {
                    unit,
                    delta,
                    modifiers,
                    ..
                } = ev
                {
                    match unit {
                        // Two-finger touchpad swipe -> move the map.
                        egui::MouseWheelUnit::Point => pan += *delta,
                        // Mouse wheel -> zoom, unless Ctrl is held (that's a zoom
                        // gesture already accounted for by `zoom_delta()` below).
                        _ if !modifiers.command && !modifiers.ctrl => wheel_zoom += delta.y,
                        _ => {}
                    }
                }
            }
            i.zoom_delta()
        });

        // Zoom (pinch / Ctrl+scroll / mouse wheel), anchored on the pointer.
        let factor = pinch * (wheel_zoom * 0.0015).exp();
        if factor != 1.0 {
            let img_before = self.to_image(origin, p);
            self.view.zoom = (self.view.zoom * factor).clamp(0.005, 200.0);
            let after = self.to_screen(origin, img_before);
            self.view.offset[0] += p.x - after.x;
            self.view.offset[1] += p.y - after.y;
        }

        // Pan from a two-finger swipe.
        self.view.offset[0] += pan.x;
        self.view.offset[1] += pan.y;
    }

    fn pan(&mut self, resp: &egui::Response) {
        let d = resp.drag_delta();
        self.view.offset[0] += d.x;
        self.view.offset[1] += d.y;
    }

    /// Hovering near the active route reports the along-track position, so the graphs
    /// show a cursor there (cross-highlight with the graphs).
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
            EditMode::Control => self.handle_control_mode(resp, origin),
        }
    }

    /// Control mode: click places/removes a course control on the map, dragging an
    /// existing control moves it, dragging empty space pans.
    fn handle_control_mode(&mut self, resp: &egui::Response, origin: egui::Pos2) {
        if resp.drag_started()
            && let Some(p) = resp.interact_pointer_pos()
        {
            self.drag = Some(match self.control_at(origin, p) {
                Some(i) => DragTarget::Control(i),
                None => DragTarget::View,
            });
        }
        if resp.dragged() {
            match self.drag {
                Some(DragTarget::Control(i)) => {
                    if let Some(p) = resp.interact_pointer_pos() {
                        let img = self.to_image(origin, p);
                        if let Some(c) = self.controls.get_mut(i) {
                            c.image_px = [img.0, img.1];
                        }
                        self.rematch_all();
                    }
                }
                _ => self.pan(resp),
            }
        }
        if resp.drag_stopped() {
            self.drag = None;
        }
        if resp.clicked() {
            self.handle_control_click(resp, origin);
        }
        if resp.secondary_clicked() {
            self.remove_control_at(resp, origin);
        }
    }

    /// Click toggles: on an existing control it removes it, elsewhere it places a
    /// new control at that map position (appended in course order).
    fn handle_control_click(&mut self, resp: &egui::Response, origin: egui::Pos2) {
        if self.map.is_none() {
            self.status = "Open a map before placing controls.".into();
            return;
        }
        let Some(p) = resp.interact_pointer_pos() else {
            return;
        };
        if let Some(i) = self.control_at(origin, p) {
            self.controls.remove(i);
            self.rematch_all();
            self.status = "Removed control.".into();
        } else {
            let img = self.to_image(origin, p);
            self.controls.push(CoursePoint {
                image_px: [img.0, img.1],
            });
            self.rematch_all();
            self.status = format!("Placed control {}.", self.controls.len());
        }
    }

    /// Right-click removes the control nearest the cursor.
    fn remove_control_at(&mut self, resp: &egui::Response, origin: egui::Pos2) {
        if let Some(p) = resp.interact_pointer_pos() {
            let best = self
                .controls
                .iter()
                .enumerate()
                .map(|(i, c)| {
                    let s = self.to_screen(origin, (c.image_px[0], c.image_px[1]));
                    (i, (s - p).length())
                })
                .filter(|&(_, d)| d < SNAP_RADIUS)
                .min_by(|a, b| a.1.total_cmp(&b.1));
            if let Some((i, _)) = best {
                self.controls.remove(i);
                self.rematch_all();
                self.status = "Removed control.".into();
            }
        }
    }

    /// Index of the course control under a screen point, if any.
    fn control_at(&self, origin: egui::Pos2, p: egui::Pos2) -> Option<usize> {
        self.controls.iter().position(|c| {
            (self.to_screen(origin, (c.image_px[0], c.image_px[1])) - p).length() < HIT_RADIUS
        })
    }

    fn handle_calibrate(&mut self, resp: &egui::Response, origin: egui::Pos2) {
        // Right-click a pin to remove it.
        if resp.secondary_clicked()
            && let Some(p) = resp.interact_pointer_pos()
            && let Some(i) = self.pin_at(origin, p)
        {
            if let Some(a) = self.active_mut() {
                a.calibration.remove(i);
            }
            self.recompute_transform_active();
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
                        if let Some(c) = self.active_mut().and_then(|a| a.calibration.get_mut(i)) {
                            c.image_px = [img.0, img.1];
                        }
                        self.recompute_transform_active();
                    }
                }
                _ => self.pan(resp),
            }
        }
        if resp.drag_stopped() {
            self.drag = None;
        }
    }

    /// Index of the active athlete's calibration pin under a screen point, if any.
    fn pin_at(&self, origin: egui::Pos2, p: egui::Pos2) -> Option<usize> {
        self.active()?.calibration.iter().position(|c| {
            (self.to_screen(origin, (c.image_px[0], c.image_px[1])) - p).length() < HIT_RADIUS
        })
    }

    /// Decide what a calibrate-mode press starts dragging.
    fn begin_calibrate_drag(&mut self, origin: egui::Pos2, p: egui::Pos2) -> DragTarget {
        // 1. Re-grab an existing pin if the press landed on one.
        if let Some(i) = self.pin_at(origin, p) {
            return DragTarget::Calibration(i);
        }
        // 2. Otherwise, if the press is on the active route, create a new pin locked
        //    to the nearest waypoint and drag it in the same gesture.
        if let Some(idx) = self.nearest_waypoint(origin, p)
            && let Some(s) = self.waypoint_screen(origin, idx)
            && (s - p).length() < SNAP_RADIUS
        {
            let img = self.to_image(origin, p);
            let pin_index = self.active_mut().map(|a| {
                a.calibration.push(CalibrationPoint {
                    track_index: idx,
                    image_px: [img.0, img.1],
                });
                a.calibration.len() - 1
            });
            if let Some(i) = pin_index {
                self.recompute_transform_active();
                self.status = "Drag onto the matching map feature, then release to lock.".into();
                return DragTarget::Calibration(i);
            }
        }
        // 3. Empty space: pan.
        DragTarget::View
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

    /// All visible athletes' routes; the active one last (on top), optionally
    /// pace-colored, the rest in their solid colors.
    fn draw_routes(&self, painter: &egui::Painter, origin: egui::Pos2) {
        let width = (self.view.zoom * 6.0).clamp(1.5, 5.0);
        for (i, a) in self.athletes.iter().enumerate() {
            if a.visible && i != self.active {
                self.draw_athlete_route(painter, origin, a, width, false);
            }
        }
        if let Some(a) = self.active()
            && a.visible
        {
            self.draw_athlete_route(painter, origin, a, width + 1.0, self.active_pace_colors);
        }
    }

    fn draw_athlete_route(
        &self,
        painter: &egui::Painter,
        origin: egui::Pos2,
        athlete: &Athlete,
        width: f32,
        pace_colors: bool,
    ) {
        let Some(t) = &athlete.transform else { return };
        if athlete.projected.len() < 2 {
            return;
        }
        let mut prev = self.to_screen(origin, t.apply(athlete.projected[0]));
        for i in 1..athlete.projected.len() {
            let cur = self.to_screen(origin, t.apply(athlete.projected[i]));
            let color = if pace_colors {
                let pace = athlete.seg_metric.get(i - 1).copied().unwrap_or(f64::NAN);
                color_for(pace, self.metric_range)
            } else {
                athlete.color
            };
            painter.line_segment([prev, cur], Stroke::new(width, color));
            prev = cur;
        }
    }

    fn draw_markers(&self, painter: &egui::Painter, origin: egui::Pos2) {
        // Calibration pins: prominent locked markers (crosshair + ring) so it's clear
        // the route point is pinned to that exact map feature. Only the active
        // athlete's pins, and only while calibrating, to keep the map clean.
        let cyan = Color32::from_rgb(0, 200, 255);
        if self.mode == EditMode::Calibrate
            && let Some(a) = self.active()
        {
            for (i, c) in a.calibration.iter().enumerate() {
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
        }
        // Cross-highlight marker from the graphs (or route hover).
        if let Some(i) = self.hover_index
            && let Some(s) = self.waypoint_screen(origin, i)
        {
            let yellow = Color32::from_rgb(255, 230, 0);
            painter.circle_stroke(s, 9.0, Stroke::new(2.5, yellow));
            painter.circle_filled(s, 3.0, yellow);
        }

        // The active athlete's start/finish.
        if let Some(a) = self.active()
            && !a.track.is_empty()
        {
            let last = a.track.len() - 1;
            for (idx, label, color) in [
                (0, "S", Color32::from_rgb(40, 170, 60)),
                (last, "F", Color32::from_rgb(200, 40, 40)),
            ] {
                if let Some(s) = self.waypoint_screen(origin, idx) {
                    painter.circle_filled(s, 8.0, color);
                    painter.circle_stroke(s, 8.0, Stroke::new(1.5, Color32::WHITE));
                    painter.text(
                        s,
                        Align2::CENTER_CENTER,
                        label,
                        FontId::proportional(11.0),
                        Color32::WHITE,
                    );
                }
            }
        }

        // Shared course controls, numbered in course order. A control the active
        // athlete's route never passes gets a warning ring.
        for (n, c) in self.controls.iter().enumerate() {
            let s = self.to_screen(origin, (c.image_px[0], c.image_px[1]));
            painter.circle_filled(s, 8.0, Color32::from_rgb(230, 30, 120));
            painter.circle_stroke(s, 8.0, Stroke::new(1.5, Color32::WHITE));
            painter.text(
                s,
                Align2::CENTER_CENTER,
                (n + 1).to_string(),
                FontId::proportional(11.0),
                Color32::WHITE,
            );
            if let Some(a) = self.active()
                && a.transform.is_some()
                && a.matched.get(n).copied().flatten().is_none()
            {
                painter.circle_stroke(s, 12.0, Stroke::new(2.0, Color32::from_rgb(255, 170, 0)));
            }
        }
    }
}
