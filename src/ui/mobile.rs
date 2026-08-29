//! The map-first mobile layout: a full-screen map with a compact top bar, a
//! persistent bottom toolbar, and at most one bottom sheet open at a time. All the
//! real content is the existing desktop sections (side panel, splits, graphs,
//! transport) re-hosted here — only the chrome around them is mobile-specific.

use crate::app::{App, EditMode, FitRequest, MobileSheet, ViewTab};
use egui::{Align2, Color32, RichText};
use std::f32::consts::FRAC_PI_2;

impl App {
    /// Enlarge interactive widgets for fingers. Cheap to set every frame; applies to
    /// both the light and dark styles.
    pub(crate) fn apply_touch_style(ctx: &egui::Context) {
        ctx.all_styles_mut(|style| {
            let s = &mut style.spacing;
            s.interact_size.y = s.interact_size.y.max(34.0);
            s.button_padding = egui::vec2(10.0, 8.0);
            s.item_spacing = egui::vec2(8.0, 8.0);
            s.slider_rail_height = s.slider_rail_height.max(10.0);
        });
    }

    /// Compose the whole narrow-screen frame.
    pub(crate) fn mobile_ui(&mut self, ui: &mut egui::Ui) {
        self.mobile_top_bar(ui);
        // Bottom toolbar is added first, so it sits at the very bottom edge.
        self.mobile_toolbar(ui);
        // A slim scrub bar rides just above the toolbar while replay is on.
        if self.tab == ViewTab::Analysis && self.playback.enabled {
            egui::Panel::bottom("m_scrub").show(ui, |ui| self.transport_minimal(ui));
        }
        // The single open sheet, above the scrub bar.
        self.mobile_sheet(ui);
        // The map fills whatever's left.
        self.map_panel(ui);
        // On-screen action buttons (touch has no keyboard shortcuts).
        self.map_fabs(ui.ctx());
        // Replay keeps ticking even though the transport lives in a sheet.
        self.advance_playback(ui.ctx());
        // A transient status toast over the top of the map.
        self.status_toast(ui.ctx());
    }

    fn mobile_top_bar(&mut self, ui: &mut egui::Ui) {
        egui::Panel::top("m_top").show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.menu_button("File", |ui| self.file_menu_items(ui));
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
    }

    /// The persistent bottom toolbar: sheet switches plus the map actions that
    /// otherwise only live in the (now hidden) side panel. Horizontally scrollable
    /// so the row is always fully reachable on the narrowest phone.
    fn mobile_toolbar(&mut self, ui: &mut egui::Ui) {
        egui::Panel::bottom("m_toolbar").show(ui, |ui| {
            egui::ScrollArea::horizontal()
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        self.sheet_toggle(ui, MobileSheet::Panel, "Panel", "Athletes & settings");
                        match self.tab {
                            ViewTab::Setup => {
                                if ui.button("Fit").on_hover_text("Fit map to view").clicked() {
                                    self.fit = Some(FitRequest::Map);
                                }
                                if ui.button("⟲").on_hover_text("Rotate 90° left").clicked() {
                                    self.rotate_by(-FRAC_PI_2);
                                }
                                if ui.button("⟳").on_hover_text("Rotate 90° right").clicked() {
                                    self.rotate_by(FRAC_PI_2);
                                }
                            }
                            ViewTab::Analysis => {
                                self.sheet_toggle(ui, MobileSheet::Splits, "Splits", "Leg splits");
                                self.sheet_toggle(ui, MobileSheet::Graphs, "Graphs", "Pace/HR/ele");
                                let on = self.draw_mode;
                                if ui
                                    .selectable_label(on, "✏ Draw")
                                    .on_hover_text("Draw route options")
                                    .clicked()
                                {
                                    self.toggle_draw_mode();
                                }
                                self.sheet_toggle(ui, MobileSheet::Transport, "Replay", "Replay");
                            }
                        }
                    });
                });
        });
    }

    /// A toolbar button that opens `which` (or closes it if already open).
    fn sheet_toggle(&mut self, ui: &mut egui::Ui, which: MobileSheet, icon: &str, tip: &str) {
        let on = self.sheet == which;
        if ui.selectable_label(on, icon).on_hover_text(tip).clicked() {
            self.sheet = if on { MobileSheet::None } else { which };
        }
    }

    /// The one open bottom sheet, hosting an existing desktop section.
    fn mobile_sheet(&mut self, ui: &mut egui::Ui) {
        // The Transport sheet is Analysis-only; fall back to closed elsewhere.
        if self.sheet == MobileSheet::Transport && self.tab != ViewTab::Analysis {
            self.sheet = MobileSheet::None;
        }
        if self.sheet == MobileSheet::None {
            return;
        }
        let title = match self.sheet {
            MobileSheet::Panel => match self.tab {
                ViewTab::Setup => "Setup",
                ViewTab::Analysis => "Analysis",
            },
            MobileSheet::Splits => "Splits",
            MobileSheet::Graphs => "Graphs",
            MobileSheet::Transport => "Replay",
            MobileSheet::None => return,
        };
        let default_h = (ui.available_height() * 0.45).clamp(160.0, 520.0);
        let mut close = false;
        egui::Panel::bottom("mobile_sheet")
            .resizable(true)
            .default_size(default_h)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.strong(title);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("✕").on_hover_text("Close").clicked() {
                            close = true;
                        }
                    });
                });
                ui.separator();
                match self.sheet {
                    MobileSheet::Panel => {
                        egui::ScrollArea::vertical().show(ui, |ui| match self.tab {
                            ViewTab::Setup => self.setup_side_panel(ui),
                            ViewTab::Analysis => self.analysis_side_panel(ui),
                        });
                    }
                    // One column per athlete — scroll sideways rather than crush.
                    MobileSheet::Splits => {
                        egui::ScrollArea::horizontal().show(ui, |ui| self.splits_content(ui));
                    }
                    MobileSheet::Graphs => self.graphs_content(ui),
                    MobileSheet::Transport => self.transport_mobile(ui),
                    MobileSheet::None => {}
                }
            });
        if close {
            self.sheet = MobileSheet::None;
        }
    }

    /// A brief status toast near the top of the map that fades a few seconds after
    /// the status text last changed (the mobile layout has no status bar).
    fn status_toast(&mut self, ctx: &egui::Context) {
        const HOLD: f64 = 3.0; // fully visible
        const FADE: f64 = 1.0; // then fade out
        let now = ctx.input(|i| i.time);
        if self.status != self.toast_text {
            self.toast_text = self.status.clone();
            self.toast_time = now;
        }
        let age = now - self.toast_time;
        if self.toast_text.is_empty() || age > HOLD + FADE {
            return;
        }
        let alpha = (((HOLD + FADE - age) / FADE).clamp(0.0, 1.0) * 235.0) as u8;
        egui::Area::new("status_toast".into())
            .anchor(Align2::CENTER_TOP, egui::vec2(0.0, 8.0))
            .interactable(false)
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style())
                    .fill(Color32::from_black_alpha(alpha.min(200)))
                    .show(ui, |ui| {
                        ui.label(
                            RichText::new(&self.toast_text)
                                .color(Color32::from_white_alpha(alpha)),
                        );
                    });
            });
        ctx.request_repaint(); // keep the fade animating
    }
}
