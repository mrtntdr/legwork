mod graphs;
mod map_view;
mod panels;

use crate::analysis::{color_for, control_indices};
use crate::app::App;
use crate::io;

impl App {
    /// Render the current map + colored route to a PNG at native resolution.
    pub(crate) fn export_png(&mut self) {
        let (Some(map), Some(t)) = (&self.map, &self.transform) else {
            self.status = "Load a map and track before exporting.".into();
            return;
        };
        let mut segments = Vec::new();
        for i in 0..self.projected.len().saturating_sub(1) {
            let a = t.apply(self.projected[i]);
            let b = t.apply(self.projected[i + 1]);
            let pace = self.seg_metric.get(i).copied().unwrap_or(f64::NAN);
            let rgba = color_for(pace, self.metric_range).to_array();
            segments.push((a, b, rgba));
        }
        let markers: Vec<(f64, f64)> = self
            .track
            .as_ref()
            .map(|track| {
                control_indices(track, &self.controls)
                    .iter()
                    .filter_map(|&idx| self.projected.get(idx).map(|&m| t.apply(m)))
                    .collect()
            })
            .unwrap_or_default();

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
