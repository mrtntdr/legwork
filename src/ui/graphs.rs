use crate::analysis::{fmt_duration, fmt_pace, quickness_color};
use crate::app::{App, EditMode};
use egui::{Align2, Color32, FontId, Rect, Sense, Stroke, pos2, vec2};

const RED: Color32 = Color32::from_rgb(240, 70, 70);
const BLUE: Color32 = Color32::from_rgb(90, 130, 255);
const HR_COLOR: Color32 = Color32::from_rgb(235, 90, 140);
const ELE_COLOR: Color32 = Color32::from_rgb(150, 170, 120);
/// Right margin reserved on every plot for the pace trim strip, so all graphs share the
/// same plot rect and their x-axes align.
const RIGHT_GUTTER: f32 = 20.0;

impl App {
    /// A bottom panel (only while placing controls, and when enabled) with pace,
    /// heart-rate and elevation graphs of the run. Coloring is controlled from the
    /// right-hand pane; the pace graph just shows the current cutoffs for reference.
    pub(crate) fn bottom_graphs(&mut self, ui: &mut egui::Ui) {
        // Graphs only appear in Controls mode; each is toggled from the right pane.
        if self.mode != EditMode::Control || !(self.show_pace || self.show_hr || self.show_ele) {
            return;
        }
        let Some(data) = self.build_graph_data() else {
            return;
        };
        let GraphData {
            pace_pts,
            pace_colors,
            hr_pts,
            ele_pts,
            marks,
            pace_cutoffs_minkm,
        } = data;
        let (show_pace, show_hr, show_ele) = (self.show_pace, self.show_hr, self.show_ele);
        let cursor = self.hover_km; // shared cursor from last frame

        // The per-control distance row is drawn only on the lowest visible graph, so the
        // whole stack shares one distance axis at the bottom.
        let hr_shown = show_hr && hr_pts.len() >= 2;
        let ele_shown = show_ele && ele_pts.len() >= 2;
        let (dist_on_pace, dist_on_hr, dist_on_ele) =
            (!hr_shown && !ele_shown, hr_shown && !ele_shown, ele_shown);

        let mut hovered: Option<f64> = None;
        let mut pace_cap = self.pace_cap_minkm;
        egui::Panel::bottom("graphs")
            .default_size(320.0)
            .resizable(true)
            .show(ui, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    if show_pace {
                        hovered = hovered.or(draw_pace_plot(
                            ui,
                            &pace_pts,
                            &pace_colors,
                            &marks,
                            pace_cutoffs_minkm,
                            cursor,
                            &mut pace_cap,
                            dist_on_pace,
                        ));
                        ui.add_space(6.0);
                    }
                    if hr_shown {
                        hovered = hovered.or(draw_plot(
                            ui,
                            "Heart rate (bpm)",
                            78.0,
                            &hr_pts,
                            HR_COLOR,
                            &marks,
                            cursor,
                            &|v| format!("{v:.0} bpm"),
                            dist_on_hr,
                        ));
                        ui.add_space(6.0);
                    }
                    if ele_shown {
                        hovered = hovered.or(draw_plot(
                            ui,
                            "Elevation (m)",
                            78.0,
                            &ele_pts,
                            ELE_COLOR,
                            &marks,
                            cursor,
                            &|v| format!("{v:.0} m"),
                            dist_on_ele,
                        ));
                    }
                });
            });
        self.pace_cap_minkm = pace_cap;
        if hovered.is_some() {
            self.pending_hover = hovered;
        }
    }

    /// Snapshot the active athlete's data for the graphs.
    fn build_graph_data(&self) -> Option<GraphData> {
        let athlete = self.active()?;
        let track = &athlete.track;
        let cum = track.cumulative_distance();
        let speeds = track.segment_speeds();

        let mut pace_pts = Vec::new();
        let mut pace_seckm = Vec::new();
        for (i, &s) in speeds.iter().enumerate() {
            if s > 0.05 {
                let seckm = 1000.0 / s;
                pace_pts.push((cum[i + 1] / 1000.0, seckm / 60.0)); // x km, y min/km
                pace_seckm.push(seckm);
            }
        }

        let range = self.metric_range;
        let pace_colors = pace_seckm
            .iter()
            .map(|&s| quickness_color(1.0 - range.normalize(s), 230))
            .collect();

        let hr_pts = track
            .points
            .iter()
            .enumerate()
            .filter_map(|(i, w)| w.hr.map(|h| (cum[i] / 1000.0, h as f64)))
            .collect();

        let ele_pts = track
            .points
            .iter()
            .enumerate()
            .filter_map(|(i, w)| w.ele.map(|e| (cum[i] / 1000.0, e)))
            .collect();

        // Vertical marks at the athlete's matched leg boundaries (start, matched
        // controls, finish); unmatched controls simply have no mark.
        let marks = athlete
            .boundaries()
            .iter()
            .filter_map(|b| b.and_then(|i| cum.get(i)).map(|&d| d / 1000.0))
            .collect();

        Some(GraphData {
            pace_pts,
            pace_colors,
            hr_pts,
            ele_pts,
            marks,
            pace_cutoffs_minkm: self.friendly_cutoffs(),
        })
    }
}

struct GraphData {
    pace_pts: Vec<(f64, f64)>,
    pace_colors: Vec<Color32>,
    hr_pts: Vec<(f64, f64)>,
    ele_pts: Vec<(f64, f64)>,
    marks: Vec<f64>,
    pace_cutoffs_minkm: (f64, f64),
}

/// Shared bounds/rect setup for a plot. Returns `(rect, plot, xmin, xmax, ymin, ymax)`.
struct PlotFrame {
    rect: Rect,
    plot: Rect,
    xmin: f64,
    xmax: f64,
    ymin: f64,
    ymax: f64,
}

impl PlotFrame {
    fn sx(&self, x: f64) -> f32 {
        self.plot.left() + ((x - self.xmin) / (self.xmax - self.xmin)) as f32 * self.plot.width()
    }
    fn sy(&self, y: f64) -> f32 {
        self.plot.bottom() - ((y - self.ymin) / (self.ymax - self.ymin)) as f32 * self.plot.height()
    }
}

/// Build a plot frame. When `ycap` is set the y-axis top is pinned to it exactly (no
/// top padding), so callers can clip the visible range to a chosen ceiling.
fn frame(rect: Rect, pts: &[(f64, f64)], extra_y: &[f64], ycap: Option<f64>) -> PlotFrame {
    let (mut xmin, mut xmax, mut ymin, mut ymax) = (f64::MAX, f64::MIN, f64::MAX, f64::MIN);
    for &(x, y) in pts {
        xmin = xmin.min(x);
        xmax = xmax.max(x);
        ymin = ymin.min(y);
        ymax = ymax.max(y);
    }
    for &y in extra_y {
        ymin = ymin.min(y);
        ymax = ymax.max(y);
    }
    if let Some(cap) = ycap {
        ymax = cap.max(ymin + 1e-6);
    }
    if (xmax - xmin).abs() < 1e-9 {
        xmax = xmin + 1.0;
    }
    if (ymax - ymin).abs() < 1e-9 {
        ymax = ymin + 1.0;
    }
    let pad = ((ymax - ymin) * 0.08).max(0.01);
    ymin -= pad;
    if ycap.is_none() {
        ymax += pad;
    }
    // A fixed right gutter is reserved on every plot (used by the pace trim strip) so
    // all graphs share the exact same plot area and their x-axes line up for hovering.
    let plot = Rect::from_min_max(
        pos2(rect.left() + 38.0, rect.top() + 4.0),
        pos2(rect.right() - RIGHT_GUTTER, rect.bottom() - 16.0),
    );
    PlotFrame {
        rect,
        plot,
        xmin,
        xmax,
        ymin,
        ymax,
    }
}

fn draw_marks(painter: &egui::Painter, f: &PlotFrame, marks: &[f64]) {
    for &m in marks {
        if m >= f.xmin && m <= f.xmax {
            let x = f.sx(m);
            painter.line_segment(
                [pos2(x, f.plot.top()), pos2(x, f.plot.bottom())],
                Stroke::new(1.0, Color32::from_gray(70)),
            );
        }
    }
}

fn draw_axis_labels(
    painter: &egui::Painter,
    f: &PlotFrame,
    yfmt: &dyn Fn(f64) -> String,
    show_xmax: bool,
) {
    let font = FontId::proportional(10.0);
    painter.text(
        pos2(f.rect.left() + 2.0, f.plot.top()),
        Align2::LEFT_TOP,
        yfmt(f.ymax),
        font.clone(),
        Color32::GRAY,
    );
    painter.text(
        pos2(f.rect.left() + 2.0, f.plot.bottom()),
        Align2::LEFT_BOTTOM,
        yfmt(f.ymin),
        font.clone(),
        Color32::GRAY,
    );
    // The total-distance label is omitted when the per-control distance row is drawn (it
    // already includes the finish), so they don't overprint.
    if show_xmax {
        painter.text(
            pos2(f.plot.right(), f.rect.bottom() - 1.0),
            Align2::RIGHT_BOTTOM,
            format!("{:.1} km", f.xmax),
            font,
            Color32::GRAY,
        );
    }
}

/// Cumulative distance (km) at each control mark, along the bottom axis. Drawn on
/// the lowest graph so the whole stack shares one distance scale.
fn draw_dist_labels(painter: &egui::Painter, f: &PlotFrame, marks: &[f64]) {
    let font = FontId::proportional(10.0);
    for &m in marks {
        if m < f.xmin - 1e-9 || m > f.xmax + 1e-9 {
            continue;
        }
        let (align, text) = if m <= f.xmin + 1e-9 {
            (Align2::LEFT_BOTTOM, format!("{m:.1}"))
        } else if m >= f.xmax - 1e-9 {
            (Align2::RIGHT_BOTTOM, format!("{m:.1} km"))
        } else {
            (Align2::CENTER_BOTTOM, format!("{m:.1}"))
        };
        painter.text(
            pos2(f.sx(m), f.rect.bottom() - 1.0),
            align,
            text,
            font.clone(),
            Color32::GRAY,
        );
    }
}

/// Draw the shared cursor (vertical line + dot on the data + value readout) at the
/// given along-track distance, if it falls within this plot.
fn draw_cursor(
    painter: &egui::Painter,
    f: &PlotFrame,
    pts: &[(f64, f64)],
    cursor_km: Option<f64>,
    fmt: &dyn Fn(f64) -> String,
) {
    let Some(cx) = cursor_km else { return };
    if cx < f.xmin || cx > f.xmax {
        return;
    }
    let x = f.sx(cx);
    painter.line_segment(
        [pos2(x, f.plot.top()), pos2(x, f.plot.bottom())],
        Stroke::new(1.0, Color32::from_gray(200)),
    );
    if let Some(&(px, py)) = pts
        .iter()
        .min_by(|a, b| (a.0 - cx).abs().total_cmp(&(b.0 - cx).abs()))
    {
        painter.circle_filled(pos2(f.sx(px), f.sy(py)), 3.0, Color32::WHITE);
        painter.text(
            pos2(f.plot.left() + 4.0, f.plot.top() + 1.0),
            Align2::LEFT_TOP,
            format!("{}  @ {:.2} km", fmt(py), px),
            FontId::proportional(11.0),
            Color32::WHITE,
        );
    }
}

/// If the pointer is over this plot, return the along-track distance (km) it points at.
fn hover_km(resp: &egui::Response, f: &PlotFrame) -> Option<f64> {
    if resp.hovered()
        && let Some(p) = resp.hover_pos()
    {
        let t = ((p.x - f.plot.left()) / f.plot.width()).clamp(0.0, 1.0) as f64;
        Some(f.xmin + t * (f.xmax - f.xmin))
    } else {
        None
    }
}

/// The pace graph: pace line colored blue→red, faint cutoff reference lines, the shared
/// cursor, and a right-edge trim strip whose thumb caps the y-axis to clip slow spikes.
/// Returns the hovered along-track distance (km), if any. `cap` (min/km) is the current
/// ceiling and is updated in place when the trim strip is dragged.
#[allow(clippy::too_many_arguments)]
fn draw_pace_plot(
    ui: &mut egui::Ui,
    pts: &[(f64, f64)],
    seg_colors: &[Color32],
    marks: &[f64],
    cutoffs_minkm: (f64, f64),
    cursor_km: Option<f64>,
    cap: &mut Option<f64>,
    label_dist: bool,
) -> Option<f64> {
    ui.label(egui::RichText::new("Pace (/km)").strong());
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 110.0), Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 3.0, Color32::from_gray(24));
    if pts.len() < 2 {
        painter.text(
            rect.center(),
            Align2::CENTER_CENTER,
            "no data",
            FontId::proportional(12.0),
            Color32::GRAY,
        );
        return None;
    }
    let (quick, slow) = cutoffs_minkm;

    // Full pace extent of the data (fixed, independent of the cap) drives both the trim
    // strip's scale and the clamp on the cap, so dragging stays stable while the plot
    // rescales. Keep at least `min_window` of range visible.
    let data_lo = pts.iter().map(|p| p.1).fold(f64::MAX, f64::min);
    let data_hi = pts.iter().map(|p| p.1).fold(f64::MIN, f64::max);
    let min_window = ((data_hi - data_lo) * 0.05).max(0.15);
    // Lowest ceiling the strip allows; `.min(data_hi)` keeps it valid (never above the
    // slowest sample) so a tiny/degenerate pace range can't invert the clamp bounds.
    let cap_lo = (data_lo + min_window).min(data_hi);
    let ceil = cap.map(|c| c.clamp(cap_lo, data_hi)).unwrap_or(data_hi);

    let f = frame(rect, pts, &[quick, slow], Some(ceil));
    // Trim strip lives in the shared right gutter, so the plot area still matches the
    // other graphs exactly.
    let strip = Rect::from_min_max(
        pos2(rect.right() - 14.0, f.plot.top()),
        pos2(rect.right() - 6.0, f.plot.bottom()),
    );

    draw_marks(&painter, &f, marks);
    // Faint cutoff reference lines (set from the right pane); skip any trimmed away.
    for (v, color) in [(quick, RED), (slow, BLUE)] {
        if v < f.ymin || v > f.ymax {
            continue;
        }
        let y = f.sy(v);
        painter.line_segment(
            [pos2(f.plot.left(), y), pos2(f.plot.right(), y)],
            Stroke::new(1.0, color.gamma_multiply(0.5)),
        );
    }
    // Pace line; spikes above the ceiling ride along the top edge.
    let clamp_y = |y: f64| y.clamp(f.ymin, f.ymax);
    for i in 0..pts.len() - 1 {
        let color = seg_colors.get(i).copied().unwrap_or(Color32::WHITE);
        painter.line_segment(
            [
                pos2(f.sx(pts[i].0), f.sy(clamp_y(pts[i].1))),
                pos2(f.sx(pts[i + 1].0), f.sy(clamp_y(pts[i + 1].1))),
            ],
            Stroke::new(1.5, color),
        );
    }
    draw_cursor(&painter, &f, pts, cursor_km, &|v| fmt_pace(v * 60.0));
    draw_axis_labels(&painter, &f, &|v| fmt_duration(v * 60.0), !label_dist);
    if label_dist {
        draw_dist_labels(&painter, &f, marks);
    }

    // Trim strip: a fixed full-range scale (top = slowest). The portion below the thumb
    // is the visible window; above it is clipped.
    let pad = ((data_hi - data_lo) * 0.05).max(0.01);
    let (s_lo, s_hi) = (data_lo - pad, data_hi + pad);
    let strip_y = |v: f64| strip.bottom() - ((v - s_lo) / (s_hi - s_lo)) as f32 * strip.height();
    painter.rect_filled(strip, 2.0, Color32::from_gray(38));
    let thumb_y = strip_y(ceil);
    painter.rect_filled(
        Rect::from_min_max(
            pos2(strip.left(), thumb_y),
            pos2(strip.right(), strip.bottom()),
        ),
        2.0,
        Color32::from_gray(66),
    );
    painter.line_segment(
        [
            pos2(strip.left() - 2.0, thumb_y),
            pos2(strip.right() + 2.0, thumb_y),
        ],
        Stroke::new(2.0, Color32::from_gray(210)),
    );
    painter.circle(
        pos2(strip.center().x, thumb_y),
        3.5,
        Color32::from_gray(210),
        Stroke::new(1.0, Color32::from_gray(40)),
    );

    let strip_resp = ui.interact(
        strip.expand2(vec2(6.0, 0.0)),
        ui.id().with("pace_trim"),
        Sense::click_and_drag(),
    );
    if strip_resp.dragged()
        && let Some(p) = strip_resp.interact_pointer_pos()
    {
        let v = s_lo + ((strip.bottom() - p.y) / strip.height()) as f64 * (s_hi - s_lo);
        let v = v.clamp(cap_lo, data_hi);
        *cap = (v < data_hi - 1e-3).then_some(v);
    }
    if strip_resp.double_clicked() {
        *cap = None;
    }
    strip_resp.on_hover_text("Drag to trim slow spikes · double-click to reset");

    hover_km(&resp, &f)
}

/// A single-color line plot (heart rate, elevation) with the shared cursor.
/// Returns the hovered along-track distance (km), if any.
#[allow(clippy::too_many_arguments)]
fn draw_plot(
    ui: &mut egui::Ui,
    title: &str,
    height: f32,
    pts: &[(f64, f64)],
    line_color: Color32,
    marks: &[f64],
    cursor_km: Option<f64>,
    fmt: &dyn Fn(f64) -> String,
    label_dist: bool,
) -> Option<f64> {
    ui.label(egui::RichText::new(title).strong());
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 3.0, Color32::from_gray(24));
    if pts.len() < 2 {
        painter.text(
            rect.center(),
            Align2::CENTER_CENTER,
            "no data",
            FontId::proportional(12.0),
            Color32::GRAY,
        );
        return None;
    }
    let f = frame(rect, pts, &[], None);
    draw_marks(&painter, &f, marks);
    for i in 0..pts.len() - 1 {
        painter.line_segment(
            [
                pos2(f.sx(pts[i].0), f.sy(pts[i].1)),
                pos2(f.sx(pts[i + 1].0), f.sy(pts[i + 1].1)),
            ],
            Stroke::new(1.5, line_color),
        );
    }
    draw_cursor(&painter, &f, pts, cursor_km, fmt);
    draw_axis_labels(&painter, &f, fmt, !label_dist);
    if label_dist {
        draw_dist_labels(&painter, &f, marks);
    }
    hover_km(&resp, &f)
}

#[cfg(test)]
mod tests {
    use super::*;

    const RECT: Rect = Rect {
        min: pos2(0.0, 0.0),
        max: pos2(400.0, 110.0),
    };

    #[test]
    fn frame_spans_the_data_with_y_padding() {
        let pts = [(0.0, 5.0), (2.0, 9.0), (10.0, 7.0)];
        let f = frame(RECT, &pts, &[], None);
        assert_eq!((f.xmin, f.xmax), (0.0, 10.0));
        assert!(
            f.ymin < 5.0 && f.ymax > 9.0,
            "y range {}..{}",
            f.ymin,
            f.ymax
        );
    }

    #[test]
    fn frame_includes_extra_y_values() {
        let pts = [(0.0, 5.0), (1.0, 6.0)];
        let f = frame(RECT, &pts, &[2.0, 12.0], None);
        assert!(f.ymin < 2.0 && f.ymax > 12.0);
    }

    #[test]
    fn frame_pins_the_top_to_ycap() {
        let pts = [(0.0, 4.0), (1.0, 20.0)];
        let f = frame(RECT, &pts, &[], Some(8.0));
        assert_eq!(f.ymax, 8.0); // exactly the cap, no top padding
        assert!(f.ymin < 4.0);
    }

    #[test]
    fn frame_widens_degenerate_ranges() {
        let pts = [(3.0, 7.0), (3.0, 7.0)];
        let f = frame(RECT, &pts, &[], None);
        assert!(f.xmax > f.xmin);
        assert!(f.ymax > f.ymin);
    }

    #[test]
    fn plot_coordinates_map_corners() {
        let pts = [(0.0, 0.0), (10.0, 100.0)];
        let f = frame(RECT, &pts, &[], None);
        assert_eq!(f.sx(f.xmin), f.plot.left());
        assert_eq!(f.sx(f.xmax), f.plot.right());
        // y axis is inverted: ymin at the bottom of the plot.
        assert_eq!(f.sy(f.ymin), f.plot.bottom());
        assert_eq!(f.sy(f.ymax), f.plot.top());
    }
}
