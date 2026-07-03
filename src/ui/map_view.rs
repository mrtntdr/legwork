use crate::analysis::color_for;
use crate::app::{App, DragTarget, EditMode, FitRequest};
use crate::athlete::Athlete;
use crate::model::{CalibrationPoint, CoursePoint};
use egui::{Align2, Color32, FontId, Rect, Sense, Stroke, pos2};

const HIT_RADIUS: f32 = 12.0;
const SNAP_RADIUS: f32 = 40.0;

impl App {
    pub(crate) fn map_panel(&mut self, ui: &mut egui::Ui) {
        egui::CentralPanel::default().show(ui, |ui| {
            // The leg strip is laid out first, so it takes its height off the top
            // and the canvas below it still owns all drag/zoom interactions.
            self.leg_strip(ui);

            let size = ui.available_size();
            let (rect, resp) = ui.allocate_exact_size(size, Sense::click_and_drag());
            let origin = rect.min;

            // Esc clears a leg selection, returning to the whole-course view.
            if self.selected_leg.is_some()
                && ui.input(|i| i.key_pressed(egui::Key::Escape))
            {
                self.select_leg(None);
            }

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
        let Some(req) = self.fit else { return };
        if rect.width() <= 1.0 {
            return;
        }
        // Target image-space box (min, size) and a margin fraction.
        let (bx, by, bw, bh, margin) = match req {
            FitRequest::Map => {
                let Some(map) = &self.map else { return };
                let (w, h) = (map.size[0] as f64, map.size[1] as f64);
                if w <= 0.0 || h <= 0.0 {
                    return;
                }
                (0.0, 0.0, w, h, 0.05)
            }
            FitRequest::Rect { min, max } => {
                // Pad degenerate (single-point) boxes so we don't divide by zero.
                let w = (max.0 - min.0).max(1.0);
                let h = (max.1 - min.1).max(1.0);
                (min.0, min.1, w, h, 0.15)
            }
        };
        let (rw, rh) = (rect.width() as f64, rect.height() as f64);
        let zoom = ((rw / bw).min(rh / bh) * (1.0 - margin)).clamp(0.005, 200.0) as f32;
        // Center the box's center in the canvas.
        let (cx, cy) = (bx + bw / 2.0, by + bh / 2.0);
        self.view.zoom = zoom;
        self.view.offset = [
            rect.width() / 2.0 - cx as f32 * zoom,
            rect.height() / 2.0 - cy as f32 * zoom,
        ];
        self.fit = None;
    }

    /// A row above the map to step through the course leg by leg: prev/next arrows,
    /// an "All" (whole course) button, and a scrollable list of leg labels. Hidden
    /// until at least one control exists.
    fn leg_strip(&mut self, ui: &mut egui::Ui) {
        if self.controls.is_empty() {
            self.selected_leg = None;
            return;
        }
        let n = self.n_legs();
        let mut pick: Option<Option<usize>> = None; // Some(None)=All, Some(Some(li))=leg
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("Leg").strong());
            // Prev: from All → last leg, else step back to All at leg 0.
            if ui.button("◀").on_hover_text("Previous leg").clicked() {
                pick = Some(match self.selected_leg {
                    None => Some(n - 1),
                    Some(0) => None,
                    Some(li) => Some(li - 1),
                });
            }
            if ui
                .selectable_label(self.selected_leg.is_none(), "All")
                .clicked()
            {
                pick = Some(None);
            }
            egui::ScrollArea::horizontal()
                .max_width(ui.available_width() - 40.0)
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        for li in 0..n {
                            let label = crate::analysis::leg_label(li, self.controls.len());
                            if ui
                                .selectable_label(self.selected_leg == Some(li), label)
                                .clicked()
                            {
                                pick = Some(Some(li));
                            }
                        }
                    });
                });
            // Next: from All → leg 0, wrap last → All.
            if ui.button("▶").on_hover_text("Next leg").clicked() {
                pick = Some(match self.selected_leg {
                    None => Some(0),
                    Some(li) if li + 1 >= n => None,
                    Some(li) => Some(li + 1),
                });
            }
        });
        if let Some(sel) = pick {
            self.select_leg(sel);
        }
        ui.separator();
    }

    /// Segment index range of athlete `a`'s route choice for leg `li` (segment `i`
    /// connects waypoints `i`..`i+1`), if both leg boundaries matched.
    fn leg_seg_range(a: &Athlete, li: usize) -> Option<std::ops::Range<usize>> {
        let b = a.boundaries();
        let from = b.get(li).copied().flatten()?;
        let to = b.get(li + 1).copied().flatten()?;
        (from < to).then_some(from..to)
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
    /// pace-colored, the rest in their solid colors. During replay the animated
    /// dots/tails replace the static routes; when a leg is selected only that
    /// leg's route choice is drawn.
    fn draw_routes(&self, painter: &egui::Painter, origin: egui::Pos2) {
        if self.playback.enabled {
            self.draw_playback(painter, origin);
            return;
        }
        let width = (self.view.zoom * 6.0).clamp(1.5, 5.0);
        for (i, a) in self.athletes.iter().enumerate() {
            if a.visible && i != self.active {
                self.draw_athlete_maybe_leg(painter, origin, a, width, false);
            }
        }
        if let Some(a) = self.active()
            && a.visible
        {
            self.draw_athlete_maybe_leg(painter, origin, a, width + 1.0, self.active_pace_colors);
        }
    }

    /// Draw an athlete's whole route, or just its selected leg's route choice.
    fn draw_athlete_maybe_leg(
        &self,
        painter: &egui::Painter,
        origin: egui::Pos2,
        a: &Athlete,
        width: f32,
        pace_colors: bool,
    ) {
        match self.selected_leg {
            Some(li) => {
                if let Some(range) = Self::leg_seg_range(a, li) {
                    self.draw_route_range(painter, origin, a, range, width, pace_colors, 1.0);
                }
            }
            None => {
                let n = a.projected.len();
                if n >= 2 {
                    self.draw_route_range(painter, origin, a, 0..n - 1, width, pace_colors, 1.0);
                }
            }
        }
    }

    /// Draw segments `seg_range` of an athlete's route (segment `i` connects
    /// waypoints `i`..`i+1`) at the given alpha multiplier.
    #[allow(clippy::too_many_arguments)]
    fn draw_route_range(
        &self,
        painter: &egui::Painter,
        origin: egui::Pos2,
        athlete: &Athlete,
        seg_range: std::ops::Range<usize>,
        width: f32,
        pace_colors: bool,
        alpha: f32,
    ) {
        let Some(t) = &athlete.transform else { return };
        let last_seg = athlete.projected.len().saturating_sub(1);
        for i in seg_range.start..seg_range.end.min(last_seg) {
            let p0 = self.to_screen(origin, t.apply(athlete.projected[i]));
            let p1 = self.to_screen(origin, t.apply(athlete.projected[i + 1]));
            let color = if pace_colors {
                let pace = athlete.seg_metric.get(i).copied().unwrap_or(f64::NAN);
                color_for(pace, self.metric_range)
            } else {
                athlete.color
            };
            let color = if alpha < 1.0 {
                color.gamma_multiply(alpha)
            } else {
                color
            };
            painter.line_segment([p0, p1], Stroke::new(width, color));
        }
    }

    /// Replay rendering: a faint base route per animated athlete, a bright tail of
    /// the last `tail_secs` behind a moving dot at the current clock. When a leg is
    /// selected everyone restarts together at that leg's start control.
    fn draw_playback(&self, painter: &egui::Painter, origin: egui::Pos2) {
        use crate::analysis::{index_at, position_at};

        let animated = self.animated_indices();
        let anchor = self.playback_anchor();
        let clock = self.playback.clock;
        let tail_secs = self.playback.tail_secs;
        let base_w = (self.view.zoom * 6.0).clamp(1.5, 5.0);

        for &i in &animated {
            let a = &self.athletes[i];
            let Some(t) = &a.transform else { continue };
            let Some(win) = self.window_for(i, anchor) else {
                continue;
            };
            let is_active = i == self.active;
            let width = if is_active { base_w + 1.0 } else { base_w };

            // The waypoint span this athlete may occupy (whole route, or the leg).
            let (lo_wp, hi_wp) = match self.selected_leg {
                Some(li) => match Self::leg_seg_range(a, li) {
                    Some(r) => (r.start, r.end),
                    None => continue,
                },
                None => (0, a.projected.len().saturating_sub(1)),
            };

            // Faint base route (whole route or leg) for context.
            if hi_wp > lo_wp {
                self.draw_route_range(painter, origin, a, lo_wp..hi_wp, width, false, 0.22);
            }

            let track_time = win.track_time(clock);
            let head = index_at(&a.timeline, track_time)
                .map(|h| h.clamp(lo_wp, hi_wp))
                .unwrap_or(lo_wp);
            let tail_start_time = if tail_secs.is_infinite() {
                win.t0
            } else {
                (track_time - tail_secs).max(win.t0)
            };
            let tail_start = index_at(&a.timeline, tail_start_time)
                .map(|s| s.clamp(lo_wp, hi_wp))
                .unwrap_or(lo_wp);

            // Bright tail up to the last passed waypoint.
            if head > tail_start {
                self.draw_route_range(painter, origin, a, tail_start..head, width + 1.0, false, 1.0);
            }

            // Interpolated dot position and the partial last segment to it.
            if let Some(m) = position_at(&a.timeline, &a.projected, track_time) {
                let dot = self.to_screen(origin, t.apply(m));
                if let Some(&hm) = a.projected.get(head) {
                    let hp = self.to_screen(origin, t.apply(hm));
                    painter.line_segment([hp, dot], Stroke::new(width + 1.0, a.color));
                }
                let r = (self.view.zoom * 7.0).clamp(4.0, 8.0);
                painter.circle_filled(dot, r, a.color);
                painter.circle_stroke(dot, r, Stroke::new(2.0, Color32::WHITE));
                // Name label when more than one athlete is animating.
                if animated.len() > 1 {
                    painter.text(
                        dot + egui::vec2(r + 3.0, -(r + 3.0)),
                        Align2::LEFT_BOTTOM,
                        &a.name,
                        FontId::proportional(12.0),
                        a.color,
                    );
                }
            }
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

        // The active athlete's start/finish. In leg view only the relevant end is
        // shown (S on the first leg, F on the last).
        let n_legs = self.n_legs();
        let show_s = self.selected_leg.is_none_or(|li| li == 0);
        let show_f = self.selected_leg.is_none_or(|li| li + 1 == n_legs);
        if let Some(a) = self.active()
            && !a.track.is_empty()
        {
            let last = a.track.len() - 1;
            for (idx, label, color, show) in [
                (0, "S", Color32::from_rgb(40, 170, 60), show_s),
                (last, "F", Color32::from_rgb(200, 40, 40), show_f),
            ] {
                if show
                    && let Some(s) = self.waypoint_screen(origin, idx)
                {
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

        // Shared course controls, numbered in course order. In leg view the two
        // controls bounding the selected leg stay prominent; the rest are dimmed.
        // A leg-relevant control the active athlete never passes gets a warning ring.
        let pink = Color32::from_rgb(230, 30, 120);
        for (n, c) in self.controls.iter().enumerate() {
            let s = self.to_screen(origin, (c.image_px[0], c.image_px[1]));
            let relevant = self
                .selected_leg
                .is_none_or(|li| n + 1 == li || n == li);
            let (r, fill, ring, text_color) = if relevant {
                let r = if self.selected_leg.is_some() { 9.0 } else { 8.0 };
                (r, pink, Color32::WHITE, Color32::WHITE)
            } else {
                (
                    7.0,
                    pink.gamma_multiply(0.4),
                    Color32::from_gray(150),
                    Color32::from_gray(210),
                )
            };
            painter.circle_filled(s, r, fill);
            painter.circle_stroke(s, r, Stroke::new(1.5, ring));
            painter.text(
                s,
                Align2::CENTER_CENTER,
                (n + 1).to_string(),
                FontId::proportional(11.0),
                text_color,
            );
            if relevant
                && let Some(a) = self.active()
                && a.transform.is_some()
                && a.matched.get(n).copied().flatten().is_none()
            {
                painter.circle_stroke(s, r + 4.0, Stroke::new(2.0, Color32::from_rgb(255, 170, 0)));
            }
        }
    }
}
