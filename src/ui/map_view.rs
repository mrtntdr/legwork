use crate::analysis::{color_for, route_midpoint_px};
use crate::app::{App, DragTarget, EditMode, FitRequest, RouteDraft, ViewTab};
use crate::athlete::{Athlete, route_color};
use crate::geo::{point_segment_dist, simplify_polyline};
use crate::model::{CalibrationPoint, CoursePoint, DrawnRoute};
use egui::{Align2, Color32, FontId, Rect, Sense, Shape, Stroke, pos2, vec2};

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
                // In draw mode, Ctrl/Cmd+Z steps back one drawing action.
                if self.draw_mode {
                    let undo =
                        ui.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::Z));
                    if undo {
                        self.undo_draw();
                    }
                }
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
            self.handle_touch_gestures(ui, origin);
            self.handle_zoom_pan(ui, &resp, origin);
            // A fresh press clears any stale tap-swallow; a long-press (touch's
            // right-click) then edits and swallows the release tap.
            if ui.input(|i| i.pointer.any_pressed()) {
                self.swallow_tap = false;
            }
            if let Some(lp) = self.long_press.update(ui, &resp) {
                self.on_long_press(origin, lp);
            }
            self.handle_interaction(&resp, origin);
            self.hover_route(&resp, origin);

            let painter = ui.painter_at(rect);
            painter.rect_filled(rect, 0.0, Color32::from_gray(30));
            self.draw_map(&painter, origin);
            self.draw_routes(&painter, origin);
            if analysis {
                self.draw_drawn_routes(&painter, origin);
            }
            self.draw_markers(&painter, origin);
        });
    }

    /// Analysis-tab keyboard shortcuts: ←/→ step legs, Esc = whole course,
    /// Space = play/pause the replay. Skipped while a text field has focus.
    fn analysis_shortcuts(&mut self, ui: &mut egui::Ui) {
        if ui.ctx().memory(|m| m.focused().is_some()) {
            return;
        }
        let (left, right, esc, space, draw_key, enter) = ui.input(|i| {
            (
                i.key_pressed(egui::Key::ArrowLeft),
                i.key_pressed(egui::Key::ArrowRight),
                i.key_pressed(egui::Key::Escape),
                i.key_pressed(egui::Key::Space),
                i.key_pressed(egui::Key::D),
                i.key_pressed(egui::Key::Enter),
            )
        });
        if draw_key {
            self.toggle_draw_mode();
        }
        if enter && self.draw_mode && let Some(draft) = self.draft.take() {
            self.finish_route(draft);
        }
        // Esc unwinds one step at a time: cancel the draft, then leave draw mode,
        // then drop the leg selection.
        if esc {
            if self.draft.is_some() {
                self.draft = None;
            } else if self.draw_mode {
                self.draw_mode = false;
            } else if self.selected_leg.is_some() {
                self.select_leg(None);
            }
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
            if ui
                .selectable_label(self.draw_mode, "✏ Draw")
                .on_hover_text("Draw route options on the map (D)")
                .clicked()
            {
                self.toggle_draw_mode();
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
        // A live touchscreen pinch/rotate is handled by `handle_touch_gestures`;
        // `zoom_delta()` here would double-count it.
        if ui.ctx().input(|i| i.multi_touch()).is_some() {
            return;
        }
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
        self.zoom_about(origin, p, factor);

        // Pan from a two-finger swipe.
        self.view.offset[0] += pan.x;
        self.view.offset[1] += pan.y;
    }

    /// Two-finger touch gestures: pinch-zoom, twist-rotate and drag-pan the map,
    /// all anchored on the gesture centroid. Sets `gesturing` so single-pointer
    /// editing (which egui synthesizes from the first touch) is suppressed and any
    /// in-flight edit drag is cancelled.
    fn handle_touch_gestures(&mut self, ui: &egui::Ui, origin: egui::Pos2) {
        if let Some(mt) = ui.ctx().input(|i| i.multi_touch()) {
            if !self.gesturing {
                self.cancel_drag();
            }
            let anchor = mt.center_pos;
            self.zoom_about(origin, anchor, mt.zoom_delta);
            self.rotate_about(origin, anchor, mt.rotation_delta);
            self.view.offset[0] += mt.translation_delta.x;
            self.view.offset[1] += mt.translation_delta.y;
            self.gesturing = true;
        } else if !ui.input(|i| i.pointer.any_down()) {
            // Only clear once every finger is lifted, so a leftover finger after a
            // pinch doesn't immediately resume editing.
            self.gesturing = false;
        }
    }

    /// Multiply the zoom by `factor`, keeping the map feature under `anchor` fixed.
    fn zoom_about(&mut self, origin: egui::Pos2, anchor: egui::Pos2, factor: f32) {
        if factor == 1.0 {
            return;
        }
        let img_before = self.to_image(origin, anchor);
        self.view.zoom = (self.view.zoom * factor).clamp(0.005, 200.0);
        let after = self.to_screen(origin, img_before);
        self.view.offset[0] += anchor.x - after.x;
        self.view.offset[1] += anchor.y - after.y;
    }

    /// Rotate the view by `delta` radians about the map feature under `anchor`
    /// (unlike the side-panel rotate, which pivots on the canvas center).
    fn rotate_about(&mut self, origin: egui::Pos2, anchor: egui::Pos2, delta: f32) {
        if delta == 0.0 {
            return;
        }
        let pivot = self.to_image(origin, anchor);
        self.view.rotation = normalize_angle(self.view.rotation + delta);
        self.center_on(origin, anchor, pivot);
    }

    /// Drop any in-flight edit drag (used when a multi-touch gesture takes over).
    fn cancel_drag(&mut self) {
        if matches!(self.drag, Some(DragTarget::RouteSketch))
            && let Some(d) = &mut self.draft
        {
            d.stroke.clear();
        }
        self.drag = None;
    }

    /// Finger-sized hit radius for markers when driving by touch.
    fn hit_radius(&self) -> f32 {
        if self.touch { 24.0 } else { HIT_RADIUS }
    }

    /// Finger-sized snap radius (create-pin / remove / end-snap) when on touch.
    fn snap_radius(&self) -> f32 {
        if self.touch { 56.0 } else { SNAP_RADIUS }
    }

    /// The image position a touch drag should place a grabbed marker at: the
    /// fingertip plus the grab offset, lifted above the finger so the target isn't
    /// hidden under it. On mouse it's just the pointer.
    fn drag_pos(&self, resp: &egui::Response) -> Option<egui::Pos2> {
        let p = resp.interact_pointer_pos()?;
        Some(if self.touch {
            p + self.grab_offset + vec2(0.0, -44.0)
        } else {
            p
        })
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
        // A two-finger gesture owns the map this frame; don't also edit.
        if self.gesturing {
            return;
        }
        match self.tab {
            // Setup: the map is editable via the current mode.
            ViewTab::Setup => match self.mode {
                EditMode::Calibrate => self.handle_calibrate(resp, origin),
                EditMode::Control => self.handle_control_mode(resp, origin),
            },
            // Analysis: read-only for the course. Draw mode sketches route options;
            // otherwise pan/zoom and clicking a control jumps to a leg.
            ViewTab::Analysis if self.draw_mode => self.handle_draw_mode(resp, origin),
            ViewTab::Analysis => {
                if resp.dragged() {
                    self.pan(resp);
                }
                // Double-tap to zoom in on empty map (not on a control).
                if self.touch
                    && resp.double_clicked()
                    && let Some(p) = resp.interact_pointer_pos()
                    && self.control_at(origin, p).is_none()
                {
                    self.zoom_about(origin, p, 2.0);
                } else if resp.clicked()
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
                Some(i) => {
                    let c = self.controls[i].image_px;
                    self.grab_offset = self.to_screen(origin, (c[0], c[1])) - p;
                    DragTarget::Control(i)
                }
                None => DragTarget::View,
            });
        }
        if resp.dragged() {
            match self.drag {
                Some(DragTarget::Control(i)) => {
                    if let Some(p) = self.drag_pos(resp) {
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
        if resp.clicked() && !self.swallow_tap {
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
            self.controls.push(CoursePoint::at(img.0, img.1));
            self.rematch_all();
            self.status = format!("Placed control {}.", self.controls.len());
        }
    }

    /// Right-click removes the control nearest the cursor.
    fn remove_control_at(&mut self, resp: &egui::Response, origin: egui::Pos2) {
        if let Some(p) = resp.interact_pointer_pos() {
            self.remove_control_near(origin, p);
        }
    }

    /// Remove the control nearest a screen point, within the snap radius.
    fn remove_control_near(&mut self, origin: egui::Pos2, p: egui::Pos2) {
        let r = self.snap_radius();
        let best = self
            .controls
            .iter()
            .enumerate()
            .map(|(i, c)| {
                let s = self.to_screen(origin, (c.image_px[0], c.image_px[1]));
                (i, (s - p).length())
            })
            .filter(|&(_, d)| d < r)
            .min_by(|a, b| a.1.total_cmp(&b.1));
        if let Some((i, _)) = best {
            self.controls.remove(i);
            self.rematch_all();
            self.status = "Removed control.".into();
        }
    }

    /// Index of the course control under a screen point, if any.
    fn control_at(&self, origin: egui::Pos2, p: egui::Pos2) -> Option<usize> {
        let r = self.hit_radius();
        self.controls.iter().position(|c| {
            (self.to_screen(origin, (c.image_px[0], c.image_px[1])) - p).length() < r
        })
    }

    fn handle_calibrate(&mut self, resp: &egui::Response, origin: egui::Pos2) {
        // Right-click a pin to remove it.
        if resp.secondary_clicked()
            && let Some(p) = resp.interact_pointer_pos()
        {
            self.remove_pin_at(origin, p);
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
                    if let Some(p) = self.drag_pos(resp) {
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

    /// Remove the active athlete's calibration pin at a screen point, if any.
    fn remove_pin_at(&mut self, origin: egui::Pos2, p: egui::Pos2) {
        if let Some(i) = self.pin_at(origin, p) {
            if let Some(a) = self.active_mut() {
                a.calibration.remove(i);
            }
            self.recompute_transform_active();
            self.status = "Removed calibration point.".into();
        }
    }

    /// Index of the active athlete's calibration pin under a screen point, if any.
    fn pin_at(&self, origin: egui::Pos2, p: egui::Pos2) -> Option<usize> {
        let r = self.hit_radius();
        self.active()?.calibration.iter().position(|c| {
            (self.to_screen(origin, (c.image_px[0], c.image_px[1])) - p).length() < r
        })
    }

    /// Decide what a calibrate-mode press starts dragging.
    fn begin_calibrate_drag(&mut self, origin: egui::Pos2, p: egui::Pos2) -> DragTarget {
        // 1. Re-grab an existing pin if the press landed on one.
        if let Some(i) = self.pin_at(origin, p) {
            let px = self
                .active()
                .and_then(|a| a.calibration.get(i))
                .map(|c| (c.image_px[0], c.image_px[1]));
            self.grab_offset = px.map_or(egui::Vec2::ZERO, |px| self.to_screen(origin, px) - p);
            return DragTarget::Calibration(i);
        }
        // 2. Otherwise, if the press is on the active route, create a new pin locked
        //    to the nearest waypoint and drag it in the same gesture.
        if let Some(idx) = self.nearest_waypoint(origin, p)
            && let Some(s) = self.waypoint_screen(origin, idx)
            && (s - p).length() < self.snap_radius()
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
                // A freshly-created pin sits at the fingertip; no grab offset.
                self.grab_offset = egui::Vec2::ZERO;
                self.recompute_transform_active();
                self.status = "Drag onto the matching map feature, then release to lock.".into();
                return DragTarget::Calibration(i);
            }
        }
        // 3. Empty space: pan.
        DragTarget::View
    }

    /// A long-press (the touch right-click) removes the marker under the finger in
    /// whatever edit mode is active, then swallows the tap egui emits on release.
    fn on_long_press(&mut self, origin: egui::Pos2, p: egui::Pos2) {
        match self.tab {
            ViewTab::Setup if self.mode == EditMode::Calibrate => {
                self.remove_pin_at(origin, p);
                self.swallow_tap = true;
                self.cancel_drag();
            }
            ViewTab::Setup => {
                self.remove_control_near(origin, p);
                self.swallow_tap = true;
                self.cancel_drag();
            }
            ViewTab::Analysis if self.draw_mode => {
                self.delete_route_vertex_at(origin, p);
                self.swallow_tap = true;
                self.cancel_drag();
            }
            _ => {}
        }
    }

    /// On-screen action buttons for touch, replacing the keyboard shortcuts the map
    /// relies on (undo, finish/cancel a route, fit). Anchored above the mobile
    /// toolbar; only shown on touch.
    pub(crate) fn map_fabs(&mut self, ctx: &egui::Context) {
        if !self.touch {
            return;
        }
        egui::Area::new(egui::Id::new("map_fabs"))
            .anchor(Align2::RIGHT_BOTTOM, vec2(-10.0, -70.0))
            .show(ctx, |ui| {
                ui.vertical(|ui| {
                    if self.tab == ViewTab::Analysis && self.draw_mode {
                        let can_finish = self.draft.as_ref().is_some_and(|d| d.points.len() >= 2);
                        if ui
                            .add_enabled(can_finish, egui::Button::new("Finish"))
                            .clicked()
                            && let Some(d) = self.draft.take()
                        {
                            self.finish_route(d);
                        }
                        if ui.button("Undo").clicked() {
                            self.undo_draw();
                        }
                        if ui.button("Cancel").clicked() {
                            if self.draft.is_some() {
                                self.draft = None;
                            } else {
                                self.draw_mode = false;
                            }
                        }
                    } else if self.tab == ViewTab::Setup
                        && self.mode == EditMode::Calibrate
                        && ui.button("Undo pin").clicked()
                        && self.active_mut().is_some_and(|a| a.calibration.pop().is_some())
                    {
                        self.recompute_transform_active();
                        self.status = "Undid last calibration point.".into();
                    }
                    if ui.button("Fit").clicked() {
                        self.fit = Some(FitRequest::Map);
                    }
                });
            });
    }

    // --- Draw mode (route options) -------------------------------------------

    /// Toggle the analysis-tab draw mode; leaving it cancels any in-progress draft.
    pub(crate) fn toggle_draw_mode(&mut self) {
        self.draw_mode = !self.draw_mode;
        if self.draw_mode {
            self.tab = ViewTab::Analysis;
            self.status =
                "Draw mode: click to drop points or drag to sketch; double-click or Enter to \
                 finish, Esc to cancel."
                    .into();
        } else {
            self.draft = None;
            self.status = "Draw mode off.".into();
        }
    }

    /// Whether a drawn route is shown/edited under the current leg selection:
    /// the whole-course view shows all routes; a leg view shows only its variants.
    fn route_visible(&self, r: &DrawnRoute, leg: Option<usize>) -> bool {
        match leg {
            None => true,
            Some(li) => r.leg == Some(li),
        }
    }

    /// The finished-route vertex under a screen point, if any (route, vertex).
    fn route_vertex_at(&self, origin: egui::Pos2, p: egui::Pos2) -> Option<(usize, usize)> {
        let leg = self.effective_leg();
        let r_hit = self.hit_radius();
        for (ri, r) in self.drawn_routes.iter().enumerate() {
            if !self.route_visible(r, leg) {
                continue;
            }
            for (vi, v) in r.points.iter().enumerate() {
                if (self.to_screen(origin, (v[0], v[1])) - p).length() < r_hit {
                    return Some((ri, vi));
                }
            }
        }
        None
    }

    /// The finished route whose polyline passes nearest a screen point (within the
    /// hit radius), for click-to-select.
    fn route_segment_at(&self, origin: egui::Pos2, p: egui::Pos2) -> Option<usize> {
        let leg = self.effective_leg();
        let mut best: Option<(usize, f64)> = None;
        let r_hit = self.hit_radius() as f64;
        for (ri, r) in self.drawn_routes.iter().enumerate() {
            if !self.route_visible(r, leg) {
                continue;
            }
            for w in r.points.windows(2) {
                let a = self.to_screen(origin, (w[0][0], w[0][1]));
                let b = self.to_screen(origin, (w[1][0], w[1][1]));
                let d = point_segment_dist([p.x as f64, p.y as f64], [a.x as f64, a.y as f64], [
                    b.x as f64, b.y as f64,
                ]);
                if d < r_hit && best.is_none_or(|(_, bd)| d < bd) {
                    best = Some((ri, d));
                }
            }
        }
        best.map(|(i, _)| i)
    }

    /// Click drops a vertex (or selects a finished route); drag on empty space
    /// sketches freehand; drag on a finished vertex moves it. Double-click/Enter
    /// finish. Panning is wheel/touchpad only here, since drag is the pen.
    fn handle_draw_mode(&mut self, resp: &egui::Response, origin: egui::Pos2) {
        if resp.drag_started()
            && let Some(p) = resp.interact_pointer_pos()
        {
            if self.draft.is_none()
                && let Some((ri, vi)) = self.route_vertex_at(origin, p)
            {
                self.selected_route = Some(ri);
                let v = self.drawn_routes[ri].points[vi];
                self.grab_offset = self.to_screen(origin, (v[0], v[1])) - p;
                self.drag = Some(DragTarget::RouteVertex {
                    route: ri,
                    vertex: vi,
                });
            } else {
                let start = self.to_image(origin, p);
                let d = self.draft.get_or_insert_with(RouteDraft::default);
                d.stroke.clear();
                d.stroke.push([start.0, start.1]);
                self.drag = Some(DragTarget::RouteSketch);
            }
        }
        if resp.dragged() {
            match self.drag {
                Some(DragTarget::RouteVertex { route, vertex }) => {
                    if let Some(p) = self.drag_pos(resp) {
                        let img = self.to_image(origin, p);
                        if let Some(v) = self
                            .drawn_routes
                            .get_mut(route)
                            .and_then(|r| r.points.get_mut(vertex))
                        {
                            *v = [img.0, img.1];
                        }
                    }
                }
                Some(DragTarget::RouteSketch) => {
                    if let Some(p) = resp.interact_pointer_pos() {
                        let img = self.to_image(origin, p);
                        let thr = (2.0 / self.view.zoom as f64).max(0.1);
                        if let Some(d) = &mut self.draft {
                            let far = d.stroke.last().is_none_or(|l| {
                                (l[0] - img.0).powi(2) + (l[1] - img.1).powi(2) > thr * thr
                            });
                            if far {
                                d.stroke.push([img.0, img.1]);
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        if resp.drag_stopped() {
            match self.drag {
                Some(DragTarget::RouteVertex { .. }) => self.recompute_drawn_stats(),
                Some(DragTarget::RouteSketch) => self.commit_stroke(),
                _ => {}
            }
            self.drag = None;
        }
        // A double-click finishes; its second click also fired `clicked` and
        // appended a duplicate final vertex, so drop that one.
        if resp.double_clicked() {
            if let Some(mut d) = self.draft.take() {
                d.points.pop();
                self.finish_route(d);
            }
            return;
        }
        if resp.clicked()
            && !self.swallow_tap
            && let Some(p) = resp.interact_pointer_pos()
        {
            if self.draft.is_none() {
                if let Some((ri, _)) = self.route_vertex_at(origin, p) {
                    self.selected_route = Some(ri);
                } else if let Some(ri) = self.route_segment_at(origin, p) {
                    self.selected_route = Some(ri);
                } else {
                    let img = self.to_image(origin, p);
                    self.draft = Some(RouteDraft {
                        points: vec![[img.0, img.1]],
                        stroke: Vec::new(),
                        checkpoints: vec![0],
                    });
                    self.selected_route = None;
                }
            } else {
                let img = self.to_image(origin, p);
                if let Some(d) = &mut self.draft {
                    d.checkpoints.push(d.points.len());
                    d.points.push([img.0, img.1]);
                }
            }
        }
        if resp.secondary_clicked()
            && let Some(p) = resp.interact_pointer_pos()
        {
            self.delete_route_vertex_at(origin, p);
        }
    }

    /// Simplify the just-finished freehand stroke and append it to the draft.
    fn commit_stroke(&mut self) {
        let tol = (2.5 / self.view.zoom as f64).max(0.25);
        if let Some(d) = &mut self.draft {
            if d.stroke.len() >= 2 {
                let simplified = simplify_polyline(&d.stroke, tol);
                d.checkpoints.push(d.points.len());
                // Skip the first sample when joining onto existing vertices.
                let skip = usize::from(!d.points.is_empty());
                d.points.extend(simplified.into_iter().skip(skip));
            }
            d.stroke.clear();
        }
    }

    /// Commit a finished draft as a drawn route, snapping its ends to the selected
    /// leg's controls when close enough.
    fn finish_route(&mut self, mut draft: RouteDraft) {
        if draft.points.len() < 2 {
            self.status = "A route needs at least two points.".into();
            return;
        }
        let leg = self.selected_leg;
        if let Some(li) = leg {
            let snap = (self.snap_radius() as f64 / self.view.zoom as f64).max(1.0);
            let snap2 = snap * snap;
            let last = draft.points.len() - 1;
            if li >= 1
                && let Some(c) = self.controls.get(li - 1)
                && dist2(draft.points[0], c.image_px) <= snap2
            {
                draft.points[0] = c.image_px;
            }
            if li < self.controls.len()
                && let Some(c) = self.controls.get(li)
                && dist2(draft.points[last], c.image_px) <= snap2
            {
                draft.points[last] = c.image_px;
            }
        }
        self.drawn_routes.push(DrawnRoute {
            points: draft.points,
            leg,
            name: String::new(),
            color: None,
        });
        self.selected_route = Some(self.drawn_routes.len() - 1);
        self.recompute_drawn_stats();
        self.status = "Added a route option.".into();
    }

    /// Right-click deletes the vertex under the cursor; a route with fewer than two
    /// vertices left is removed entirely.
    fn delete_route_vertex_at(&mut self, origin: egui::Pos2, p: egui::Pos2) {
        if let Some((ri, vi)) = self.route_vertex_at(origin, p) {
            let emptied = self
                .drawn_routes
                .get_mut(ri)
                .map(|r| {
                    r.points.remove(vi);
                    r.points.len() < 2
                })
                .unwrap_or(false);
            if emptied {
                self.drawn_routes.remove(ri);
                if self.selected_route == Some(ri) {
                    self.selected_route = None;
                }
                self.status = "Removed a route option.".into();
            }
            self.recompute_drawn_stats();
        }
    }

    /// Step back one drawing action: shrink the draft to the last checkpoint, or
    /// remove the most recently finished route.
    fn undo_draw(&mut self) {
        if let Some(d) = &mut self.draft {
            match d.checkpoints.pop() {
                Some(cp) => {
                    d.points.truncate(cp);
                    if d.points.is_empty() {
                        self.draft = None;
                    }
                }
                None => self.draft = None,
            }
        } else if self.drawn_routes.pop().is_some() {
            self.selected_route = None;
            self.recompute_drawn_stats();
            self.status = "Removed last route option.".into();
        }
    }

    /// Dashed drawn routes, the live draft, and (for the selected route) vertex
    /// handles, a length/points label, and rings on collected controls.
    fn draw_drawn_routes(&self, painter: &egui::Painter, origin: egui::Pos2) {
        let leg = self.effective_leg();
        let scored = self.controls.iter().any(|c| c.score.is_some());
        let width = (self.view.zoom * 6.0).clamp(1.5, 5.0);
        for (i, r) in self.drawn_routes.iter().enumerate() {
            if !self.route_visible(r, leg) {
                continue;
            }
            let color = route_color(r, i);
            let selected = self.selected_route == Some(i);
            self.draw_dashed(painter, origin, &r.points, width, color, selected);

            if selected && self.draw_mode {
                for v in &r.points {
                    let s = self.to_screen(origin, (v[0], v[1]));
                    painter.circle_filled(s, 4.0, color);
                    painter.circle_stroke(s, 4.0, Stroke::new(1.5, Color32::WHITE));
                }
            }
            if let Some(mid) = route_midpoint_px(&r.points) {
                let s = self.to_screen(origin, (mid[0], mid[1]));
                let text = route_label(self.drawn_stats.get(i), scored);
                draw_label(painter, s, &text);
            }
            if selected
                && let Some(st) = self.drawn_stats.get(i)
            {
                for &ci in &st.collected {
                    if let Some(c) = self.controls.get(ci) {
                        let s = self.to_screen(origin, (c.image_px[0], c.image_px[1]));
                        painter.circle_stroke(s, 13.0, Stroke::new(2.5, color));
                    }
                }
            }
        }
        // The in-progress draft: committed vertices + live stroke + a rubber band
        // to the cursor, in the next palette color.
        if let Some(d) = &self.draft {
            let color = crate::athlete::ATHLETE_COLORS
                [self.drawn_routes.len() % crate::athlete::ATHLETE_COLORS.len()];
            let mut pts = d.points.clone();
            pts.extend(d.stroke.iter().copied());
            if let Some(hp) = painter.ctx().pointer_hover_pos()
                && d.stroke.is_empty()
                && !pts.is_empty()
            {
                let img = self.to_image(origin, hp);
                pts.push([img.0, img.1]);
            }
            self.draw_dashed(painter, origin, &pts, width, color, true);
            for v in &d.points {
                let s = self.to_screen(origin, (v[0], v[1]));
                painter.circle_filled(s, 4.0, color);
            }
        }
    }

    /// Draw a pixel-space polyline as a dashed screen line (a lone point as a dot),
    /// with a faint white under-stroke when selected.
    fn draw_dashed(
        &self,
        painter: &egui::Painter,
        origin: egui::Pos2,
        pts: &[[f64; 2]],
        width: f32,
        color: Color32,
        selected: bool,
    ) {
        let screen: Vec<egui::Pos2> =
            pts.iter().map(|v| self.to_screen(origin, (v[0], v[1]))).collect();
        if screen.len() < 2 {
            if let Some(&s) = screen.first() {
                painter.circle_filled(s, width.max(2.0), color);
            }
            return;
        }
        if selected {
            painter.add(Shape::line(
                screen.clone(),
                Stroke::new(width + 3.0, Color32::from_rgba_unmultiplied(255, 255, 255, 70)),
            ));
        }
        let dash = (width * 2.0).max(8.0);
        let gap = width.max(5.0);
        painter.extend(Shape::dashed_line(&screen, Stroke::new(width, color), dash, gap));
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
        // Touch precision-drag guide: while dragging a marker with a finger, the
        // marker rides above the fingertip; a thin line ties them together so it's
        // clear what's being moved.
        if self.touch
            && let Some(fp) = painter.ctx().pointer_interact_pos()
        {
            let target = match self.drag {
                Some(DragTarget::Control(i)) => {
                    self.controls.get(i).map(|c| (c.image_px[0], c.image_px[1]))
                }
                Some(DragTarget::Calibration(i)) => self
                    .active()
                    .and_then(|a| a.calibration.get(i))
                    .map(|c| (c.image_px[0], c.image_px[1])),
                Some(DragTarget::RouteVertex { route, vertex }) => self
                    .drawn_routes
                    .get(route)
                    .and_then(|r| r.points.get(vertex))
                    .map(|v| (v[0], v[1])),
                _ => None,
            };
            if let Some(t) = target {
                let s = self.to_screen(origin, t);
                painter.line_segment([fp, s], Stroke::new(1.0, Color32::from_white_alpha(140)));
                painter.circle_stroke(s, 11.0, Stroke::new(2.0, Color32::from_rgb(0, 200, 255)));
            }
        }

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
            if let Some(score) = c.score {
                painter.text(
                    s + vec2(0.0, r + 1.0),
                    Align2::CENTER_TOP,
                    format!("{score}p"),
                    FontId::proportional(10.0),
                    Color32::from_rgb(255, 220, 120),
                );
            }
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

/// Squared distance between two image-pixel points.
fn dist2(a: [f64; 2], b: [f64; 2]) -> f64 {
    (a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)
}

/// A drawn route's on-map label: its length, plus points and points/km when the
/// course is scored (rogaine).
fn route_label(stats: Option<&crate::analysis::RouteStats>, scored: bool) -> String {
    let Some(s) = stats else {
        return "— m".into();
    };
    let len = s
        .length_m
        .map(|m| format!("{m:.0} m"))
        .unwrap_or_else(|| "— m".into());
    if scored && s.points > 0 {
        match s.length_m.filter(|&m| m > 0.0) {
            Some(m) => format!("{len} · {} p · {:.1} p/km", s.points, s.points as f64 / (m / 1000.0)),
            None => format!("{len} · {} p", s.points),
        }
    } else {
        len
    }
}

/// A small text label with a dark rounded backing, centered on `pos`.
fn draw_label(painter: &egui::Painter, pos: egui::Pos2, text: &str) {
    let galley = painter.layout_no_wrap(text.to_owned(), FontId::proportional(12.0), Color32::WHITE);
    let rect = Rect::from_center_size(pos, galley.size() + vec2(8.0, 4.0));
    painter.rect_filled(rect, 3.0, Color32::from_rgba_unmultiplied(0, 0, 0, 190));
    painter.galley(rect.min + vec2(4.0, 2.0), galley, Color32::WHITE);
}

/// Wrap an angle (radians) into `(-π, π]`, so accumulated 90° taps stay in a tidy
/// range for the rotation slider.
fn normalize_angle(a: f32) -> f32 {
    use std::f32::consts::{PI, TAU};
    let a = a.rem_euclid(TAU);
    if a > PI { a - TAU } else { a }
}
