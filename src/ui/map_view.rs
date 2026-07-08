use crate::analysis::color_for;
use crate::app::{App, DragTarget, EditMode, FitRequest, ViewTab};
use crate::athlete::Athlete;
use crate::model::{CalibrationPoint, CoursePoint};
use egui::{Align2, Color32, FontId, Rect, Sense, Stroke, pos2};

const HIT_RADIUS: f32 = 12.0;
const SNAP_RADIUS: f32 = 40.0;

impl App {
    pub(crate) fn map_panel(&mut self, ui: &mut egui::Ui) {
        egui::CentralPanel::default().show(ui, |ui| {
            let analysis = self.tab == ViewTab::Analysis;
            // The leg strip is laid out first, so it takes its height off the top
            // and the canvas below it still owns all drag/zoom interactions.
            if analysis {
                self.leg_strip(ui);
            }

            let size = ui.available_size();
            let (rect, resp) = ui.allocate_exact_size(size, Sense::click_and_drag());
            let origin = rect.min;

            if analysis {
                self.analysis_shortcuts(ui);
            } else {
                // Ctrl/Cmd+Z removes the active athlete's latest calibration point.
                let undo = ui.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::Z));
                if undo && self.active_mut().is_some_and(|a| a.calibration.pop().is_some()) {
                    self.recompute_transform_active();
                    self.status = "Undid last calibration point.".into();
                }
            }
            self.apply_pending_rotation(rect);
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

    /// Analysis-tab keyboard shortcuts: ←/→ step legs, Esc = whole course,
    /// Space = play/pause the replay. Skipped while a text field has focus.
    fn analysis_shortcuts(&mut self, ui: &mut egui::Ui) {
        if ui.ctx().memory(|m| m.focused().is_some()) {
            return;
        }
        let (left, right, esc, space) = ui.input(|i| {
            (
                i.key_pressed(egui::Key::ArrowLeft),
                i.key_pressed(egui::Key::ArrowRight),
                i.key_pressed(egui::Key::Escape),
                i.key_pressed(egui::Key::Space),
            )
        });
        if esc && self.selected_leg.is_some() {
            self.select_leg(None);
        }
        if !self.controls.is_empty() {
            let n = self.n_legs();
            if left {
                self.select_leg(match self.selected_leg {
                    None => Some(n - 1),
                    Some(0) => None,
                    Some(li) => Some(li - 1),
                });
            }
            if right {
                self.select_leg(match self.selected_leg {
                    None => Some(0),
                    Some(li) if li + 1 >= n => None,
                    Some(li) => Some(li + 1),
                });
            }
        }
        if space && self.playback.enabled {
            let total = self.playback_total();
            if !self.playback.playing && self.playback.clock >= total {
                self.playback.clock = 0.0;
            }
            self.playback.playing = !self.playback.playing;
        }
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
        // The box, rotated by the current view angle, spans a larger screen-aligned
        // rectangle; fit that so a rotated map/leg still lands fully inside the canvas.
        let (s, c) = self.view.rotation.sin_cos();
        let (s, c) = (s.abs() as f64, c.abs() as f64);
        let span_w = c * bw + s * bh;
        let span_h = s * bw + c * bh;
        let zoom = ((rw / span_w).min(rh / span_h) * (1.0 - margin)).clamp(0.005, 200.0) as f32;
        self.view.zoom = zoom;
        // Center the box's center in the canvas (accounting for rotation).
        let center = rect.center();
        self.center_on(rect.min, center, (bx + bw / 2.0, by + bh / 2.0));
        self.fit = None;
    }

    /// Apply a pending view rotation, pivoting about the canvas center so the map
    /// spins in place rather than swinging off screen.
    fn apply_pending_rotation(&mut self, rect: Rect) {
        let Some(target) = self.pending_rotate else {
            return;
        };
        if rect.width() <= 1.0 {
            return; // No canvas yet — keep the request for a later frame.
        }
        self.pending_rotate = None;
        let origin = rect.min;
        let center = rect.center();
        // The image point under the canvas center must stay put across the rotation.
        let pivot = self.to_image(origin, center);
        self.view.rotation = normalize_angle(target);
        self.center_on(origin, center, pivot);
    }

    /// Set `view.offset` so image point `img` lands at screen point `at` under the
    /// current zoom and rotation.
    fn center_on(&mut self, origin: egui::Pos2, at: egui::Pos2, img: (f64, f64)) {
        let (sin, cos) = self.view.rotation.sin_cos();
        let x = img.0 as f32 * self.view.zoom;
        let y = img.1 as f32 * self.view.zoom;
        self.view.offset = [
            at.x - origin.x - (cos * x - sin * y),
            at.y - origin.y - (sin * x + cos * y),
        ];
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

    /// The leg selection as it applies to map rendering — only the Analysis tab
    /// isolates legs; Setup always shows whole routes.
    fn effective_leg(&self) -> Option<usize> {
        (self.tab == ViewTab::Analysis)
            .then_some(self.selected_leg)
            .flatten()
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
        match self.tab {
            // Setup: the map is editable via the current mode.
            ViewTab::Setup => match self.mode {
                EditMode::Calibrate => self.handle_calibrate(resp, origin),
                EditMode::Control => self.handle_control_mode(resp, origin),
            },
            // Analysis: read-only — pan/zoom, and clicking a control jumps to a leg.
            ViewTab::Analysis => {
                if resp.dragged() {
                    self.pan(resp);
                }
                if resp.clicked()
                    && let Some(p) = resp.interact_pointer_pos()
                    && let Some(k) = self.control_at(origin, p)
                {
                    // Control k bounds legs k (into it) and k+1 (out of it):
                    // click selects the incoming leg, click again for the outgoing.
                    let li = if self.selected_leg == Some(k) { k + 1 } else { k };
                    self.select_leg(Some(li));
                }
            }
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
        let Some(map) = &self.map else { return };
        let (w, h) = (map.size[0] as f64, map.size[1] as f64);
        // A textured quad through the four rotated image corners, so the map turns
        // with the view (`painter.image` only draws axis-aligned rectangles).
        let corners = [(0.0, 0.0), (w, 0.0), (w, h), (0.0, h)];
        let uvs = [
            pos2(0.0, 0.0),
            pos2(1.0, 0.0),
            pos2(1.0, 1.0),
            pos2(0.0, 1.0),
        ];
        let mut mesh = egui::Mesh::with_texture(map.texture.id());
        for (corner, uv) in corners.iter().zip(uvs) {
            mesh.vertices.push(egui::epaint::Vertex {
                pos: self.to_screen(origin, *corner),
                uv,
                color: Color32::WHITE,
            });
        }
        mesh.add_triangle(0, 1, 2);
        mesh.add_triangle(0, 2, 3);
        painter.add(egui::Shape::mesh(mesh));
    }

    /// All visible athletes' routes; the active one last (on top), optionally
    /// pace-colored, the rest in their solid colors. During replay the animated
    /// dots/tails replace the static routes; when a leg is selected only that
    /// leg's route choice is drawn.
    fn draw_routes(&self, painter: &egui::Painter, origin: egui::Pos2) {
        if self.playback.enabled && self.tab == ViewTab::Analysis {
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
        match self.effective_leg() {
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
            let (lo_wp, hi_wp) = match self.effective_leg() {
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
        // athlete's pins, and only while calibrating in Setup, to keep the map clean.
        let cyan = Color32::from_rgb(0, 200, 255);
        if self.tab == ViewTab::Setup
            && self.mode == EditMode::Calibrate
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
        let leg = self.effective_leg();
        let show_s = leg.is_none_or(|li| li == 0);
        let show_f = leg.is_none_or(|li| li + 1 == n_legs);
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
            let relevant = leg.is_none_or(|li| n + 1 == li || n == li);
            let (r, fill, ring, text_color) = if relevant {
                let r = if leg.is_some() { 9.0 } else { 8.0 };
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

/// Wrap an angle (radians) into `(-π, π]`, so accumulated 90° taps stay in a tidy
/// range for the rotation slider.
fn normalize_angle(a: f32) -> f32 {
    use std::f32::consts::{PI, TAU};
    let a = a.rem_euclid(TAU);
    if a > PI { a - TAU } else { a }
}
