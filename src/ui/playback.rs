use crate::analysis::{ClockMode, fmt_duration};
use crate::app::{App, StartMode};
use egui::RichText;

impl App {
    /// The Analysis tab's bottom bar: the replay transport group plus the
    /// Splits/Graphs drawer toggles. Also advances the replay clock while playing.
    pub(crate) fn playback_bar(&mut self, ui: &mut egui::Ui) {
        egui::Panel::bottom("playback").show(ui, |ui| {
            ui.horizontal(|ui| {
                // Drawer toggles live at the far right, outside the replay group,
                // so the Splits/Graphs drawers are reachable with Replay off too.
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.toggle_value(&mut self.show_splits, "Splits")
                        .on_hover_text("Leg-by-leg comparison table");
                    ui.toggle_value(&mut self.show_graphs, "Graphs")
                        .on_hover_text("Pace / heart-rate / elevation graphs");
                    ui.separator();
                    ui.with_layout(
                        egui::Layout::left_to_right(egui::Align::Center),
                        |ui| self.transport_controls(ui),
                    );
                });
            });
        });

        self.advance_playback(ui.ctx());
    }

    /// Advance the replay clock while playing, stopping at the end. Runs every frame
    /// regardless of which layout drew the transport, so replay keeps ticking on
    /// mobile where the transport lives in a sheet.
    pub(crate) fn advance_playback(&mut self, ctx: &egui::Context) {
        if self.playback.enabled && self.playback.playing {
            let total = self.playback_total();
            let dt = ctx.input(|i| i.stable_dt) as f64;
            self.playback.clock += dt * self.playback.speed;
            if self.playback.clock >= total {
                self.playback.clock = total;
                self.playback.playing = false;
            }
            ctx.request_repaint();
        }
    }

    /// The replay group of the transport bar: enable, play/pause, scrub timeline,
    /// speed, start mode, tail length and solo.
    fn transport_controls(&mut self, ui: &mut egui::Ui) {
        ui.checkbox(&mut self.playback.enabled, "Replay")
            .on_hover_text("Animate the routes over time");
        if !self.playback.enabled {
            self.playback.playing = false;
            ui.label(
                RichText::new("play the routes back as moving dots")
                    .weak()
                    .small(),
            );
            return;
        }

        let total = self.playback_total();
        if total <= 0.0 {
            self.playback.playing = false;
            ui.label(RichText::new("tracks need timestamps to replay").weak());
            return;
        }

        // Play / pause (restart if parked at the end).
        let icon = if self.playback.playing { "⏸" } else { "⏵" };
        if ui.button(icon).on_hover_text("Play / pause").clicked() {
            if self.playback.clock >= total {
                self.playback.clock = 0.0;
            }
            self.playback.playing = !self.playback.playing;
        }

        ui.label(RichText::new(self.playback_readout(total)).monospace());

        // Right-aligned options; the timeline fills whatever's left.
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            self.transport_settings(ui);
            // Timeline scrubber fills the remaining width.
            ui.spacing_mut().slider_width = (ui.available_width() - 12.0).max(80.0);
            ui.add(egui::Slider::new(&mut self.playback.clock, 0.0..=total).show_value(false));
        });
    }

    /// The replay option controls (solo, tail, start mode, speed), layout-agnostic
    /// so they pack right-to-left in the desktop bar or stack in the mobile sheet.
    fn transport_settings(&mut self, ui: &mut egui::Ui) {
        ui.checkbox(&mut self.playback.solo, "Solo")
            .on_hover_text("Animate only the active athlete");

        let mut full = self.playback.tail_secs.is_infinite();
        if ui
            .checkbox(&mut full, "Full tail")
            .on_hover_text("Show the whole route travelled so far")
            .changed()
        {
            self.playback.tail_secs = if full { f64::INFINITY } else { 60.0 };
        }
        if !full {
            ui.add(
                egui::Slider::new(&mut self.playback.tail_secs, 5.0..=600.0)
                    .logarithmic(true)
                    .suffix(" s")
                    .fixed_decimals(0),
            )
            .on_hover_text("Tail length");
        }

        // Start mode — locked while a leg is selected (leg replay always
        // restarts everyone together at the leg's start control).
        let leg_locked = self.selected_leg.is_some();
        let resp = ui
            .add_enabled_ui(!leg_locked, |ui| {
                egui::ComboBox::from_id_salt("start_mode")
                    .selected_text(if leg_locked {
                        "Leg restart"
                    } else {
                        match self.playback.mode {
                            StartMode::MassStart => "Mass start",
                            StartMode::RealTime => "Real time",
                        }
                    })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(
                            &mut self.playback.mode,
                            StartMode::MassStart,
                            "Mass start",
                        );
                        ui.selectable_value(
                            &mut self.playback.mode,
                            StartMode::RealTime,
                            "Real time",
                        );
                    });
            })
            .response;
        if leg_locked {
            resp.on_hover_text("Leg replay restarts everyone at the control");
        }

        ui.add(
            egui::Slider::new(&mut self.playback.speed, 1.0..=120.0)
                .logarithmic(true)
                .suffix("×")
                .fixed_decimals(0),
        )
        .on_hover_text("Playback speed");
        ui.label("Speed");
    }

    /// A slim scrub bar (play/pause + readout + timeline) shown above the mobile
    /// toolbar while replay is on, so the route can be scrubbed without opening the
    /// Transport sheet. Renders nothing until the timeline has length.
    pub(crate) fn transport_minimal(&mut self, ui: &mut egui::Ui) {
        let total = self.playback_total();
        if total <= 0.0 {
            return;
        }
        ui.horizontal(|ui| {
            let icon = if self.playback.playing { "⏸" } else { "⏵" };
            if ui.button(icon).clicked() {
                if self.playback.clock >= total {
                    self.playback.clock = 0.0;
                }
                self.playback.playing = !self.playback.playing;
            }
            ui.label(RichText::new(self.playback_readout(total)).monospace());
            ui.spacing_mut().slider_width = (ui.available_width() - 12.0).max(60.0);
            ui.add(egui::Slider::new(&mut self.playback.clock, 0.0..=total).show_value(false));
        });
    }

    /// The full transport laid out for a mobile bottom sheet: enable, a play row, a
    /// full-width timeline, then the options wrapped to as many rows as they need.
    pub(crate) fn transport_mobile(&mut self, ui: &mut egui::Ui) {
        ui.checkbox(&mut self.playback.enabled, "Replay")
            .on_hover_text("Animate the routes over time");
        if !self.playback.enabled {
            self.playback.playing = false;
            ui.label(
                RichText::new("Play the routes back as moving dots.")
                    .weak()
                    .small(),
            );
            return;
        }
        let total = self.playback_total();
        if total <= 0.0 {
            self.playback.playing = false;
            ui.label(RichText::new("Tracks need timestamps to replay.").weak());
            return;
        }
        ui.horizontal(|ui| {
            let icon = if self.playback.playing { "⏸" } else { "⏵" };
            if ui.button(icon).on_hover_text("Play / pause").clicked() {
                if self.playback.clock >= total {
                    self.playback.clock = 0.0;
                }
                self.playback.playing = !self.playback.playing;
            }
            ui.label(RichText::new(self.playback_readout(total)).monospace());
        });
        ui.spacing_mut().slider_width = (ui.available_width() - 12.0).max(80.0);
        ui.add(egui::Slider::new(&mut self.playback.clock, 0.0..=total).show_value(false));
        ui.separator();
        ui.horizontal_wrapped(|ui| self.transport_settings(ui));
    }

    /// The transport time readout: `clock / total` for mass-start/leg replays, or
    /// the wall-clock time for real-time replays.
    fn playback_readout(&self, total: f64) -> String {
        let clock = self.playback.clock;
        if let ClockMode::RealTime = self.playback_clock_mode()
            && let Some(anchor) = self.playback_anchor()
        {
            let t = anchor + chrono::Duration::milliseconds((clock * 1000.0) as i64);
            return t.format("%H:%M:%S").to_string();
        }
        format!("{} / {}", fmt_duration(clock), fmt_duration(total))
    }
}
