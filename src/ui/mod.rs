mod graphs;
mod leg_analysis;
mod map_view;
mod panels;

use crate::analysis::color_for;
use crate::app::App;
use crate::io;

impl App {
    /// Render the current map with every visible athlete's route (and the shared
    /// course controls) to a PNG at native resolution.
    pub(crate) fn export_png(&mut self) {
        let Some(map) = &self.map else {
            self.status = "Load a map and track before exporting.".into();
            return;
        };
        let mut segments = Vec::new();
        // Non-active athletes first so the active route draws on top, matching
        // the on-screen layering.
        let order = (0..self.athletes.len())
            .filter(|&i| i != self.active)
            .chain(std::iter::once(self.active));
        for i in order {
            let Some(a) = self.athletes.get(i) else { continue };
            let (Some(t), true) = (&a.transform, a.visible) else {
                continue;
            };
            let pace_colors = i == self.active && self.active_pace_colors;
            for k in 0..a.projected.len().saturating_sub(1) {
                let p0 = t.apply(a.projected[k]);
                let p1 = t.apply(a.projected[k + 1]);
                let rgba = if pace_colors {
                    let pace = a.seg_metric.get(k).copied().unwrap_or(f64::NAN);
                    color_for(pace, self.metric_range).to_array()
                } else {
                    a.color.to_array()
                };
                segments.push((p0, p1, rgba));
            }
        }
        if segments.is_empty() {
            self.status = "Load a map and track before exporting.".into();
            return;
        }
        let markers: Vec<(f64, f64)> = self
            .controls
            .iter()
            .map(|c| (c.image_px[0], c.image_px[1]))
            .collect();

        match io::render_png(&map.bytes, &segments, &markers) {
            Ok(png) => {
                if let Some(path) = rfd::FileDialog::new()
                    .add_filter("PNG", &["png"])
                    .set_file_name("route.png")
                    .save_file()
                {
                    match std::fs::write(&path, png) {
                        Ok(()) => self.status = "Exported PNG.".into(),
                        Err(e) => self.status = format!("Export failed: {e}"),
                    }
                }
            }
            Err(e) => self.status = format!("Export failed: {e}"),
        }
    }
}
