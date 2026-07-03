use crate::analysis::{ClockMode, fmt_duration};
use crate::app::{App, StartMode};
use egui::RichText;

impl App {
    /// The replay transport bar (Map tab): enable, play/pause, a scrub timeline,
    /// speed, start mode, tail length, and a solo toggle. Also advances the clock
    /// while playing.
    pub(crate) fn playback_bar(&mut self, ui: &mut egui::Ui) {
        egui::Panel::bottom("playback").show(ui, |ui| {
            ui.horizontal(|ui| {
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
                    ui.label(
                        RichText::new("tracks need timestamps to replay").weak(),
                    );
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

                    // Timeline scrubber fills the remaining width.
                    ui.spacing_mut().slider_width = (ui.available_width() - 12.0).max(80.0);
                    ui.add(
                        egui::Slider::new(&mut self.playback.clock, 0.0..=total).show_value(false),
                    );
                });
            });
        });

        // Advance the clock while playing; stop at the end.
        if self.playback.enabled && self.playback.playing {
            let total = self.playback_total();
            let dt = ui.ctx().input(|i| i.stable_dt) as f64;
            self.playback.clock += dt * self.playback.speed;
            if self.playback.clock >= total {
                self.playback.clock = total;
                self.playback.playing = false;
            }
            ui.ctx().request_repaint();
        }
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
