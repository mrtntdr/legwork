use crate::analysis::{LegRow, compare, fmt_duration, fmt_pace};
use crate::app::App;
use egui::{Color32, RichText};
use egui_extras::{Column, TableBuilder};

const BEST_GREEN: Color32 = Color32::from_rgb(80, 210, 120);

impl App {
    /// The Splits drawer (Analysis tab): a resizable bottom panel with the
    /// leg-by-leg comparison table of all visible athletes, on demand so the map
    /// keeps center stage.
    pub(crate) fn splits_drawer(&mut self, ui: &mut egui::Ui) {
        if !self.show_splits {
            return;
        }
        egui::Panel::bottom("splits")
            .resizable(true)
            .default_size(230.0)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.strong("Splits");
                    ui.checkbox(&mut self.show_cumulative, "Cumulative times");
                    if self.controls.is_empty() {
                        ui.label(
                            RichText::new(
                                "place controls in Setup · Course to split the course into legs",
                            )
                            .weak(),
                        );
                    }
                });

                let visible: Vec<usize> = (0..self.athletes.len())
                    .filter(|&i| self.athletes[i].visible)
                    .collect();
                if visible.is_empty() {
                    ui.label(
                        RichText::new("Add a GPS track (and tick it visible) to compare legs.")
                            .weak(),
                    );
                    return;
                }

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

                self.comparison_table(ui, &visible, &rows);
            });
    }

    fn comparison_table(&mut self, ui: &mut egui::Ui, visible: &[usize], rows: &[LegRow]) {
        let show_cum = self.show_cumulative;
        let row_h = if show_cum { 48.0 } else { 34.0 };
        // Deferred so the click can mutate `self` after the table closure returns.
        let mut jump_to: Option<usize> = None;

        let mut table = TableBuilder::new(ui)
            .striped(true)
            .column(Column::exact(46.0));
        for _ in visible {
            table = table.column(Column::auto().at_least(130.0));
        }
        table
            .header(22.0, |mut h| {
                h.col(|ui| {
                    ui.strong("Leg");
                });
                for &i in visible {
                    let a = &self.athletes[i];
                    h.col(|ui| {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new("●").color(a.color));
                            let name = RichText::new(&a.name);
                            ui.label(if i == self.active {
                                name.strong().underline()
                            } else {
                                name.strong()
                            });
                        });
                    });
                }
            })
            .body(|mut body| {
                for (li, row) in rows.iter().enumerate() {
                    body.row(row_h, |mut r| {
                        r.col(|ui| {
                            if ui
                                .selectable_label(false, RichText::new(&row.label).strong())
                                .on_hover_text("Show this leg on the map")
                                .clicked()
                            {
                                jump_to = Some(li);
                            }
                        });
                        let best_secs = row
                            .best
                            .and_then(|b| row.cells[b].leg.as_ref())
                            .and_then(|l| l.duration_secs);
                        let best_cum = row
                            .best_cum
                            .and_then(|b| row.cells[b].cum_secs);
                        for (ci, cell) in row.cells.iter().enumerate() {
                            r.col(|ui| {
                                leg_cell(
                                    ui,
                                    cell,
                                    row.best == Some(ci),
                                    best_secs,
                                    best_cum,
                                    show_cum,
                                );
                            });
                        }
                    });
                }
                self.total_row(&mut body, visible, row_h);
            });

        if let Some(li) = jump_to {
            // The map is right behind the drawer — just zoom it to the leg.
            self.select_leg(Some(li));
        }
    }

    /// A final row with each athlete's full start→finish time and gap to the winner.
    fn total_row(&self, body: &mut egui_extras::TableBody<'_>, visible: &[usize], row_h: f32) {
        let totals: Vec<Option<f64>> = visible
            .iter()
            .map(|&i| self.athletes[i].track.duration_secs())
            .collect();
        let best = totals
            .iter()
            .filter_map(|t| *t)
            .min_by(f64::total_cmp);
        body.row(row_h, |mut r| {
            r.col(|ui| {
                ui.strong("Total");
            });
            for t in &totals {
                r.col(|ui| match t {
                    Some(secs) => {
                        let is_best = best.is_some_and(|b| (secs - b).abs() < 0.5);
                        ui.horizontal(|ui| {
                            let text = RichText::new(fmt_duration(*secs)).strong();
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
                        });
                    }
                    None => {
                        ui.label("–");
                    }
                });
            }
        });
    }
}

/// One athlete's cell for one leg: time (+delta to best), pace · length, and
/// optionally the cumulative time (+gap to the leader).
fn leg_cell(
    ui: &mut egui::Ui,
    cell: &crate::analysis::compare::LegCell,
    is_best: bool,
    best_secs: Option<f64>,
    best_cum: Option<f64>,
    show_cum: bool,
) {
    let Some(leg) = &cell.leg else {
        ui.centered_and_justified(|ui| {
            ui.label(RichText::new("–").weak());
        });
        return;
    };
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = 1.0;
        ui.horizontal(|ui| {
            match leg.duration_secs {
                Some(secs) => {
                    let text = RichText::new(fmt_duration(secs)).strong();
                    ui.label(if is_best { text.color(BEST_GREEN) } else { text });
                    if let Some(b) = best_secs
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
            };
        });
        let pace = leg
            .pace_s_per_km
            .map(fmt_pace)
            .unwrap_or_else(|| "–".into());
        ui.label(
            RichText::new(format!("{pace} · {:.0} m", leg.route_length))
                .weak()
                .small(),
        )
        .on_hover_text(format!("Detour vs straight line: {:+.0}%", leg.detour_pct));
        if show_cum {
            match cell.cum_secs {
                Some(cum) => {
                    let leader = best_cum.is_some_and(|b| (cum - b).abs() < 0.5);
                    let gap = best_cum
                        .filter(|_| !leader)
                        .map(|b| format!(" (+{})", fmt_duration(cum - b)))
                        .unwrap_or_default();
                    let text = RichText::new(format!("Σ {}{gap}", fmt_duration(cum))).small();
                    ui.label(if leader { text.color(BEST_GREEN) } else { text.weak() });
                }
                None => {
                    ui.label(RichText::new("Σ –").weak().small());
                }
            }
        }
    });
}
